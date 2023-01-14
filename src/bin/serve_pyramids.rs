extern crate twitch_piramid_bot;

use askama::Template;
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
    Router,
};
use rocksdb::{IteratorMode, DB};
use std::collections::HashMap;
use std::net::SocketAddr;

#[tokio::main]
async fn main() {
    // build our application with some routes
    let app = Router::new().route("/", get(serve));

    // run it
    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await
        .unwrap();
}

async fn serve() -> impl IntoResponse {
    let db = DB::open_default("pyramids.db").unwrap();
    let mut last = String::new();
    let mut data = HashMap::new();
    let mut channel_data: Vec<(String, u32)> = Vec::new();
    for item in db.iterator(IteratorMode::Start) {
        let (key, value) = item.unwrap();
        let key = String::from_utf8(key.into_vec()).unwrap();
        let mut parts = key.split(" ");
        let channel = parts.next().unwrap();
        let person = parts.next().unwrap();
        let value: u32 = String::from_utf8(value.into_vec())
            .unwrap()
            .parse()
            .unwrap();

        if last != channel {
            if last != "" {
                channel_data.sort_by(|(_, a), (_, b)| a.cmp(b));
                channel_data.reverse();
                data.insert(last.to_string(), channel_data);
                channel_data = Vec::new();
            }
            last = channel.to_string();
        }
        channel_data.push((person.to_string(), value));
    }
    channel_data.sort_by(|(_, a), (_, b)| a.cmp(b));
    channel_data.reverse();
    data.insert(last.to_string(), channel_data);
    let template = PyramidsTemplate { data };
    HtmlTemplate(template)
}

#[derive(Template)]
#[template(path = "main.html")]
struct PyramidsTemplate {
    data: HashMap<String, Vec<(String, u32)>>,
}

struct HtmlTemplate<T>(T);

impl<T> IntoResponse for HtmlTemplate<T>
where
    T: Template,
{
    fn into_response(self) -> Response {
        match self.0.render() {
            Ok(html) => Html(html).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to render template. Error: {}", err),
            )
                .into_response(),
        }
    }
}
