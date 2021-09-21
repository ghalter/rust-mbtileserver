use std::collections::HashMap;

use hyper::header::{HeaderValue, CONTENT_ENCODING, CONTENT_TYPE, HOST};
use hyper::{Body, Request, Response, StatusCode};

use regex::Regex;
use std::sync::{Arc, RwLock};
use serde_json::json;

use crate::tiles::{get_grid_data, get_tile_data, TileMeta, TileSummaryJSON};
use crate::utils::{encode, get_blank_image, DataFormat};

lazy_static! {
    static ref TILE_URL_RE: Regex =
        Regex::new(r"^/services/(?P<tile_path>.*)/tiles/(?P<z>\d+)/(?P<x>\d+)/(?P<y>\d+)\.(?P<format>[a-zA-Z]+)/?(\?(?P<query>.*))?").unwrap();
}

#[allow(dead_code)]
static INTERNAL_SERVER_ERROR: &[u8] = b"Internal Server Error";
static FORBIDDEN: &[u8] = b"Forbidden";
static NOT_FOUND: &[u8] = b"Not Found";
static NO_CONTENT: &[u8] = b"";

fn not_found() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(NOT_FOUND.into())
        .unwrap()
}

fn no_content() -> Response<Body> {
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(NO_CONTENT.into())
        .unwrap()
}

fn forbidden() -> Response<Body> {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .body(FORBIDDEN.into())
        .unwrap()
}

#[allow(dead_code)]
fn server_error() -> Response<Body> {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .body(INTERNAL_SERVER_ERROR.into())
        .unwrap()
}

fn bad_request(msg: String) -> Response<Body> {
    Response::builder()
        .status(StatusCode::BAD_REQUEST)
        .body(Body::from(msg))
        .unwrap()
}

pub fn tile_map() -> Response<Body> {
    let css = include_str!("../templates/static/dist/core.min.css");
    let js = include_str!("../templates/static/dist/core.min.js");
    let html = include_str!("../templates/map.html");
    let body = Body::from(html.replace("{{css}}", css).replace("{{js}}", js));
    Response::new(body)
}

fn is_host_valid(host: Option<&HeaderValue>, allowed_hosts: &Vec<String>) -> bool {
    if host.is_none() {
        return false;
    }

    let host = host.unwrap().to_str().unwrap().split(':').next().unwrap();

    for pattern in allowed_hosts.iter() {
        if pattern == "*" || pattern == host {
            return true;
        }
        if pattern.starts_with(".") {
            let mut pattern = pattern.clone();
            let pattern = pattern.split_off(1);
            if host.ends_with(&pattern) {
                return true;
            }
        }
    }

    false
}

pub struct SharedData {
    pub tileset: HashMap<String, TileMeta>,
}

pub async fn get_service(
    request: Request<Body>,
    allowed_hosts: Vec<String>,
    headers: Vec<(String, String)>,
    disable_preview: bool,
    shared: Arc<RwLock<SharedData>>,
    subdomain: String,
) -> Result<Response<Body>, hyper::Error> {
    if !is_host_valid(request.headers().get(HOST), &allowed_hosts) {
        return Ok(forbidden());
    };

    let uri = request.uri();

    let path = uri.path().replace("/api/tileserver/", "/");

    let scheme = match uri.scheme_str() {
        Some(scheme) => format!("{}://", scheme),
        None => String::from("https://"),
    };
    let base_url = format!(
        "{}{}{}/services", scheme, request.headers()["host"].to_str().unwrap(), subdomain
    );


    let tilesets = shared.read().unwrap().tileset.clone();


    match TILE_URL_RE.captures(&path) {
        Some(matches) => {
            let tile_path = matches.name("tile_path").unwrap().as_str();
            let tile_meta = tilesets.get(tile_path).unwrap();
            let z = matches.name("z").unwrap().as_str().parse::<u32>().unwrap();
            let x = matches.name("x").unwrap().as_str().parse::<u32>().unwrap();
            let y = matches.name("y").unwrap().as_str().parse::<u32>().unwrap();
            let y: u32 = (1 << z) - 1 - y;
            let data_format = matches.name("format").unwrap().as_str();
            // For future use
            let _query_string = match matches.name("query") {
                Some(q) => q.as_str(),
                None => "",
            };

            let mut response = Response::builder();
            for (k, v) in headers {
                response = response.header(&k, &v);
            }

            return match data_format {
                "json" => match tile_meta.grid_format {
                    Some(grid_format) => match get_grid_data(
                        &tile_meta.connection_pool.get().unwrap(),
                        grid_format,
                        z,
                        x,
                        y,
                    ) {
                        Ok(data) => {
                            let data = serde_json::to_vec(&data).unwrap();
                            Ok(response
                                .header(CONTENT_TYPE, DataFormat::JSON.content_type())
                                .header(CONTENT_ENCODING, "gzip")
                                .body(Body::from(encode(&data)))
                                .unwrap())
                        }
                        Err(_) => Ok(no_content()),
                    },
                    None => Ok(not_found()),
                },
                "pbf" => match get_tile_data(&tile_meta.connection_pool.get().unwrap(), z, x, y) {
                    Ok(data) => Ok(response
                        .header(CONTENT_TYPE, DataFormat::PBF.content_type())
                        .header(CONTENT_ENCODING, "gzip")
                        .body(Body::from(data))
                        .unwrap()),
                    Err(_) => Ok(no_content()),
                },
                _ => {
                    let data =
                        match get_tile_data(&tile_meta.connection_pool.get().unwrap(), z, x, y) {
                            Ok(data) => data,
                            Err(_) => get_blank_image(),
                        };
                    Ok(response
                        .header(CONTENT_TYPE, DataFormat::new(data_format).content_type())
                        .body(Body::from(data))
                        .unwrap())
                }
            };
        }
        None => {
            if path.starts_with("/services") {

                let mut response = Response::builder();
                for (k, v) in headers {
                    response = response.header(&k, &v);
                }

                let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
                if segments.len() == 1 {
                    // Root url (/services): show all services
                    let mut tiles_summary = Vec::new();
                    for (tile_name, tile_meta) in tilesets {
                        tiles_summary.push(TileSummaryJSON {
                            image_type: tile_meta.tile_format,
                            url: format!("{}/{}", base_url, tile_name),
                        });
                    }
                    let resp_json = serde_json::to_string(&tiles_summary).unwrap(); // TODO handle error
                    return Ok(response
                        .header(CONTENT_TYPE, "application/json")
                        .body(Body::from(resp_json))
                        .unwrap()); // TODO handle error
                }

                // Tileset details (/services/<tileset-path>)
                let tile_name = segments[1..].join("/");
                let tile_meta = match tilesets.get(&tile_name) {
                    Some(tile_meta) => tile_meta.clone(),
                    None => {
                        if segments[segments.len() - 1] == "map" {
                            // Tileset map preview (/services/<tileset-path>/map)
                            let tile_name = segments[1..segments.len() - 1].join("/");
                            match tilesets.get(&tile_name) {
                                Some(_) => {
                                    if disable_preview {
                                        return Ok(not_found());
                                    }
                                    return Ok(tile_map());
                                }
                                None => {
                                    return Ok(bad_request(format!(
                                        "Tileset does not exist: {}",
                                        tile_name
                                    )));
                                }
                            }
                        }
                        return Ok(bad_request(format!(
                            "Tileset does not exist: {}",
                            tile_name
                        )));
                    }
                };
                let query_string = match request.uri().query() {
                    Some(q) => format!("?{}", q),
                    None => String::new(),
                };

                let mut tile_meta_json = json!({
                    "name": tile_meta.name,
                    "version": tile_meta.version,
                    "tiles": vec![format!(
                        "{}/{}/tiles/{{z}}/{{x}}/{{y}}.{}{}",
                        base_url,
                        tile_name,
                        tile_meta.tile_format.format(),
                        query_string
                    )],
                    "tilejson": tile_meta.tilejson,
                    "scheme": tile_meta.scheme,
                    "id": tile_meta.id,
                    "format": tile_meta.tile_format,
                    "grids": match tile_meta.grid_format {
                        Some(_) => Some(vec![format!(
                            "{}/{}/tiles/{{z}}/{{x}}/{{y}}.json{}",
                            base_url, tile_name, query_string
                        )]),
                        None => None,
                    },
                    "bounds": tile_meta.bounds,
                    "center": tile_meta.center,
                    "minzoom": tile_meta.minzoom,
                    "maxzoom": tile_meta.maxzoom,
                    "description": tile_meta.description,
                    "attribution": tile_meta.attribution,
                    "type": tile_meta.layer_type,
                    "legend": tile_meta.legend,
                    "template": tile_meta.template,
                });
                match tile_meta.json {
                    Some(json_data) => {
                        for (k, v) in json_data.as_object().unwrap() {
                            tile_meta_json[k] = v.clone();
                        }
                    }
                    None => (),
                };
                if !disable_preview {
                    tile_meta_json["map"] = json!(format!("{}/{}/{}", base_url, tile_name, "map"));
                }

                return Ok(response
                    .header(CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_string(&tile_meta_json).unwrap()))
                    .unwrap()); // TODO handle error
            }
        }
    };
    Ok(not_found())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiles::discover_tilesets;
    use crate::utils::decode;
    use hyper::{body};
    use serde_json;
    use serde_json::Value as JSONValue;
    use std::path::PathBuf;

    async fn setup(
        host: &str,
        path: &str,
        allowed_hosts: Option<Vec<String>>,
        headers: Option<Vec<(String, String)>>,
        disable_preview: bool,
    ) -> Response<Body> {
        let request = Request::get(path)
            .header("host", host)
            .body(Body::from(""))
            .unwrap();
        let subdomain = "";
        let tilesets = discover_tilesets(String::new(), PathBuf::from("./tiles"));
        get_service(
            request,
            tilesets,
            allowed_hosts.unwrap_or(vec![String::from("*")]),
            headers.unwrap_or(vec![]),
            disable_preview,
            subdomain.clone(),
        )
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn get_services() {
        let response = setup("localhost", "/services", None, None, false).await;
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn forbidden_domain() {
        let response = setup(
            "localhost",
            "/services",
            Some(vec![String::from("example.com")]),
            None,
            false,
        )
            .await;
        assert_eq!(response.status(), 403);
    }

    #[tokio::test]
    async fn allowed_all_domains() {
        let response = setup(
            "example.com",
            "/services",
            Some(vec![String::from("*")]),
            None,
            false,
        )
            .await;
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn allowed_domain() {
        let response = setup(
            "example.com",
            "/services",
            Some(vec![String::from("example.com")]),
            None,
            false,
        )
            .await;
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn forbidden_subdomain() {
        let response = setup(
            "test.example.com",
            "/services",
            Some(vec![String::from("example.com")]),
            None,
            false,
        )
            .await;
        assert_eq!(response.status(), 403);
    }

    #[tokio::test]
    async fn allowed_subdomain() {
        let response = setup(
            "test.example.com",
            "/services",
            Some(vec![String::from(".example.com")]),
            None,
            false,
        )
            .await;
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn get_details() {
        let response = setup(
            "localhost",
            "/services/geography-class-png",
            None,
            None,
            false,
        )
            .await;
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn get_preview_map() {
        let response = setup(
            "localhost",
            "/services/geography-class-png/map",
            None,
            None,
            false,
        )
            .await;
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn get_existing_tile() {
        let response = setup(
            "localhost",
            "/services/geography-class-png/tiles/0/0/0.png",
            None,
            None,
            false,
        )
            .await;
        assert_eq!(response.status(), 200);
    }

    #[tokio::test]
    async fn get_non_existing_tile() {
        // Geography Class PNG has no tiles beyond zoom level 1 and should return a blank image
        let response = setup(
            "localhost",
            "/services/geography-class-png/tiles/2/0/0.png",
            None,
            None,
            false,
        )
            .await;
        assert_eq!(response.status(), 200);
        assert_eq!(
            body::to_bytes(response.into_body()).await.unwrap(),
            get_blank_image()
        );
    }

    #[tokio::test]
    async fn get_existing_utfgrid_data() {
        let response = setup(
            "localhost",
            "/services/geography-class-png/tiles/0/0/0.json",
            None,
            None,
            false,
        )
            .await;
        assert_eq!(response.status(), 200);
        let data: JSONValue = serde_json::from_str(
            &decode(
                body::to_bytes(response.into_body()).await.unwrap().to_vec(),
                DataFormat::GZIP,
            )
                .unwrap(),
        )
            .unwrap();
        assert_ne!(data.get("data"), None);
        assert_ne!(data.get("grid"), None);
        assert_ne!(data.get("keys"), None);
    }

    #[tokio::test]
    async fn get_non_existing_utfgrid_data() {
        // should return empty content with 204 status
        let response = setup(
            "localhost",
            "/services/geography-class-png/tiles/2/0/0.json",
            None,
            None,
            false,
        )
            .await;
        assert_eq!(response.status(), 204);
    }

    #[tokio::test]
    async fn disable_preview() {
        let response = setup(
            "localhost",
            "/services/geography-class-png/map",
            None,
            None,
            true,
        )
            .await;
        assert_eq!(response.status(), 404);
    }
}
