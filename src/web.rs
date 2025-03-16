use crate::state_manager::Command;
use crate::teams::{Member, Queue, Status, Team};
use askama::Template;
use axum::extract::State;
use axum::{
    Router, extract,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
};
use log::info;
use serde::Deserialize;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

pub async fn create_webserver(sender: Sender<Command>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let app = Router::new()
            .route("/", get(main_handler))
            .route("/channel/{channel}", get(queue_handler))
            .route("/channel/{channel}/queue", get(queue_fragment))
            .route("/channel/{channel}/queue/{team_num}", get(team_fragment))
            .route("/create/queue", post(create_queue))
            .with_state(sender);

        // run it
        let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await.unwrap();
        info!("listening on {}", listener.local_addr().unwrap());
        axum::serve(listener, app).await.unwrap();
    })
}

async fn main_handler() -> Html<&'static str> {
    Html("<h1>Hello, World!</h1>")
}

async fn queue_handler(
    extract::Path(channel): extract::Path<String>,
    State(state): State<Sender<Command>>,
) -> impl IntoResponse {
    info!("Web for channel: {}", channel);
    let (tx, rx) = oneshot::channel();
    let _ = state
        .send(Command::ShowQueue {
            channel: channel.to_string(),
            resp: tx,
        })
        .await;
    let queue = rx.await.unwrap_or_else(|_| Queue::default());
    let template = MainTemplate { queue, channel };
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

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct CreateInput {
    channel: String,
    queue_size: u8,
    team_size: u8,
}

async fn create_queue(
    State(state): State<Sender<Command>>,
    extract::Form(input): extract::Form<CreateInput>,
) -> impl IntoResponse {
    info!("Updating teams for {}", input.channel);
    let _ = state
        .send(Command::CreateQueue {
            channel: input.channel.clone(),
            teams: input.queue_size,
            per_team: input.team_size,
        })
        .await;
    let (tx, rx) = oneshot::channel();
    let _ = state
        .send(Command::ShowQueue {
            channel: input.channel,
            resp: tx,
        })
        .await;
    let queue = rx.await.unwrap_or_else(|_| Queue::default());
    let template = QueueTemplate { queue };
    HtmlTemplate(template)
}

#[derive(Template)]
#[template(path = "main.html")]
struct MainTemplate {
    queue: Queue,
    channel: String,
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
