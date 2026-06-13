//! Integration tests for the `#[api_router]` module macro.

use serde::Serialize;
use tork::{api_router, get, Method};

#[derive(Serialize, schemars::JsonSchema)]
struct Widget {
    id: i64,
}

#[api_router(prefix = "/widgets", tags = ["widgets"])]
mod widgets_router {
    use super::*;

    #[get("/", summary = "List widgets")]
    pub async fn list() -> tork::Result<Vec<Widget>> {
        Ok(Vec::new())
    }

    #[get("/{id}", summary = "Get a widget")]
    pub async fn get_one(id: i64) -> tork::Result<Widget> {
        Ok(Widget { id })
    }
}

#[test]
fn api_router_collects_module_routes() {
    let routes = widgets_router::router().into_routes();

    assert_eq!(routes.len(), 2);

    let paths: Vec<&str> = routes.iter().map(|route| route.path()).collect();
    assert!(paths.contains(&"/widgets"), "paths: {paths:?}");
    assert!(paths.contains(&"/widgets/{id}"), "paths: {paths:?}");

    for route in &routes {
        assert_eq!(route.method(), &Method::GET);
        assert!(route.meta().tags.contains(&"widgets".to_owned()));
    }
}
