use crate::teams::{Member, Queue, Status, Team};
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
        let app = Router::new()
            .route("/", get(main_handler))
            .route("/channel/{channel}", get(queue_handler))
            .route("/channel/{channel}/queue", get(queue_fragment))
            .route("/channel/{channel}/queue/{team_num}", get(team_fragment));

        // run it
        let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await.unwrap();
        info!("listening on {}", listener.local_addr().unwrap());
        axum::serve(listener, app).await.unwrap();
    })
}

async fn main_handler() -> Html<&'static str> {
    Html("<h1>Hello, World!</h1>")
}

async fn queue_handler(extract::Path(channel): extract::Path<String>) -> impl IntoResponse {
    info!("Web for channel: {}", channel);
    let queue = Queue {
        size: 2,
        team_size: 2,
        teams: vec![
            Team {
                members: vec![Member {
                    name: "lol".to_string(),
                    status: Status::Confirmed,
                }],
            },
            Team { members: vec![] },
        ],
    };
    let template = MainTemplate { queue };
    HtmlTemplate(template)
}

async fn queue_fragment(extract::Path(channel): extract::Path<String>) -> impl IntoResponse {
    info!("Fragment for channel: {channel}");
    let queue = Queue {
        size: 2,
        team_size: 2,
        teams: vec![
            Team {
                members: vec![Member {
                    name: "lol".to_string(),
                    status: Status::Confirmed,
                }],
            },
            Team { members: vec![] },
        ],
    };
    let template = QueueTemplate { queue };
    HtmlTemplate(template)
}

async fn team_fragment(extract::Path(params): extract::Path<(String, usize)>) -> impl IntoResponse {
    info!("Fragment for team {} in channel: {}", params.1, params.0);
    let team = Team {
        members: vec![Member {
            name: "lol".to_string(),
            status: Status::Confirmed,
        }],
    };
    let template = TeamTemplate {
        team,
        team_number: params.1,
        team_size: 2,
    };
    HtmlTemplate(template)
}

#[derive(Template)]
#[template(path = "main.html")]
struct MainTemplate {
    queue: Queue,
}

#[derive(Template)]
#[template(path = "queue.html")]
struct QueueTemplate {
    queue: Queue,
}

#[derive(Template)]
#[template(path = "team.html")]
struct TeamTemplate {
    team: Team,
    team_size: u8,
    team_number: usize,
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
                format!("Failed to render template. Error: {err}"),
            )
                .into_response(),
        }
    }
}
