use askama::Template;
use axum::{
    Router, extract,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
};
use log::info;
use tokio::task::JoinHandle;

pub async fn create_webserver() -> JoinHandle<()> {
    tokio::spawn(async move {
        let app = Router::new().route("/", get(handler));

        // run it
        let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await.unwrap();
        info!("listening on {}", listener.local_addr().unwrap());
        axum::serve(listener, app).await.unwrap();
    })
}

async fn handler() -> impl IntoResponse {
    let template = HelloTemplate {};
    HtmlTemplate(template)
}

#[derive(Template)]
#[template(path = "layout.html")]
struct HelloTemplate {}

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
                format!("Failed to render template. Error: {err}"),
            )
                .into_response(),
        }
    }
}
