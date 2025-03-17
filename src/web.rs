use crate::state_manager::Command;
use crate::teams::{AddResult, DeletionResult, Member, Queue, Status, Team};
use axum::extract::State;
use axum::routing::delete;
use axum::{
    Router, extract,
    http::StatusCode,
    response::Html,
    routing::{get, post},
};
use log::info;
use minijinja::{Environment, context, path_loader};
use serde::Deserialize;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[derive(Clone)]
struct AppState {
    engine: Environment<'static>,
    db: Sender<Command>,
}
pub async fn create_webserver(sender: Sender<Command>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut env = Environment::new();
        env.set_loader(path_loader("templates"));

        let app_state = AppState {
            engine: env,
            db: sender,
        };

        let app = Router::new()
            .route("/", get(main_handler))
            .route("/channel/{channel}", get(queue_handler))
            .route("/channel/{channel}/queue", get(queue_fragment))
            .route(
                "/channel/{channel}/queue/{team_num}",
                get(team_fragment).post(add_to_queue),
            )
            .route("/channel/{channel}/queue/{team_num}/{user}", delete(delete_to_queue))
            .route("/create/queue", post(create_queue))
            .with_state(app_state);

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
    State(state): State<AppState>,
) -> Result<Html<String>, StatusCode> {
    info!("Web for channel: {}", channel);
    let (tx, rx) = oneshot::channel();
    let _ = state
        .db
        .send(Command::ShowQueue {
            channel: channel.to_string(),
            resp: tx,
        })
        .await;
    let queue = rx.await.unwrap_or_else(|_| Queue::default());
    let template = state
        .engine
        .get_template("main.html")
        .unwrap()
        .render(context! {queue => queue, channel => channel});
    Ok(Html(template.unwrap()))
}

async fn queue_fragment(
    State(state): State<AppState>,
    extract::Path(channel): extract::Path<String>,
) -> Result<Html<String>, StatusCode> {
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
    let template = state
        .engine
        .get_template("queue.html")
        .unwrap()
        .render(context! {queue => queue});
    Ok(Html(template.unwrap()))
}

async fn team_fragment(
    State(state): State<AppState>,
    extract::Path(params): extract::Path<(String, usize)>,
) -> Result<Html<String>, StatusCode> {
    info!("Fragment for team {} in channel: {}", params.1, params.0);
    let team = Team {
        members: vec![Member {
            name: "lol".to_string(),
            status: Status::Confirmed,
        }],
    };

    let template = state
        .engine
        .get_template("team.html")
        .unwrap()
        .render(context! {team => team, team_number => params.1, team_size => 2});
    Ok(Html(template.unwrap()))
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct CreateInput {
    channel: String,
    queue_size: u8,
    team_size: u8,
}

async fn create_queue(
    State(state): State<AppState>,
    extract::Form(input): extract::Form<CreateInput>,
) -> Result<Html<String>, StatusCode> {
    info!("Updating teams for {}", input.channel);
    let _ = state
        .db
        .send(Command::CreateQueue {
            channel: input.channel.clone(),
            teams: input.queue_size,
            per_team: input.team_size,
        })
        .await;
    let (tx, rx) = oneshot::channel();
    let _ = state
        .db
        .send(Command::ShowQueue {
            channel: input.channel,
            resp: tx,
        })
        .await;
    let queue = rx.await.unwrap_or_else(|_| Queue::default());
    let template = state
        .engine
        .get_template("queue.html")
        .unwrap()
        .render(context! {queue => queue});
    Ok(Html(template.unwrap()))
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct AddInput {
    user: String,
}

async fn add_to_queue(
    state: State<AppState>,
    extract::Path((channel, team_num)): extract::Path<(String, usize)>,
    extract::Form(input): extract::Form<AddInput>,
) -> Result<Html<String>, (StatusCode, Html<String>)> {
    info!("Adding to queue {} in channel {}", team_num, channel);
    let (tx, rx) = oneshot::channel();
    let _ = state
        .db
        .send(Command::AddToQueue {
            channel: channel.clone(),
            user: input.user.clone(),
            second_user: None,
            team: Some(team_num as u8),
            resp: tx,
        })
        .await;
    match rx.await.unwrap() {
        AddResult::AlreadyInQueue | AddResult::NoSpace => {
            let template = state
                .engine
                .get_template("error.html")
                .unwrap()
                .render(context! {act => true, msg => "Ya esta en algun equipo"});
            return Err((StatusCode::CONFLICT, Html(template.unwrap())));
        }
        AddResult::GeneralError => {
            let template = state
                .engine
                .get_template("error.html")
                .unwrap()
                .render(context! {act => true, msg => "Error en el servidor"});
            return Err((StatusCode::INTERNAL_SERVER_ERROR, Html(template.unwrap())));
        }
        _ => {}
    };

    let (tx, rx) = oneshot::channel();
    let _ = state
        .db
        .send(Command::ShowQueue {
            channel: channel.clone(),
            resp: tx,
        })
        .await;
    let queue = rx.await.unwrap_or_else(|_| Queue::default());

    let template = state.engine.get_template("team.html").unwrap().render(
        context! {team => queue.teams[team_num], team_number => team_num, team_size => queue.team_size, channel=> channel, clear => true},
    );
    Ok(Html(template.unwrap()))
}

async fn delete_to_queue(
    state: State<AppState>,
    extract::Path((channel, team_num, user)): extract::Path<(String, usize, String)>,
) -> Result<Html<String>, (StatusCode, Html<String>)> {
    info!("Adding to queue {} in channel {}", team_num, channel);
    let (tx, rx) = oneshot::channel();
    let _ = state
        .db
        .send(Command::RemoveFromQueue {
            channel: channel.clone(),
            user: user.clone(),
            resp: tx,
        })
        .await;
    match rx.await.unwrap() {
        DeletionResult::GeneralError => {
            let template = state
                .engine
                .get_template("error.html")
                .unwrap()
                .render(context! {act => true, msg => "Error en el servidor"});
            return Err((StatusCode::INTERNAL_SERVER_ERROR, Html(template.unwrap())));
        }
        DeletionResult::NotFound => {
            let template = state
                .engine
                .get_template("error.html")
                .unwrap()
                .render(context! {act => true, msg => "Como seleccionaste a alguien que no esta en algun equipo?"});
            return Err((StatusCode::CONFLICT, Html(template.unwrap())));
        }
        DeletionResult::Success => {}
    };

    let (tx, rx) = oneshot::channel();
    let _ = state
        .db
        .send(Command::ShowQueue {
            channel: channel.clone(),
            resp: tx,
        })
        .await;
    let queue = rx.await.unwrap_or_else(|_| Queue::default());

    let template = state.engine.get_template("team.html").unwrap().render(
        context! {team => queue.teams[team_num], team_number => team_num, team_size => queue.team_size, channel=> channel},
    );
    Ok(Html(template.unwrap()))
}
