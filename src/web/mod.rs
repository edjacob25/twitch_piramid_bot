use crate::state::Source;
use crate::state::command::Command;
use crate::state::data::*;
use async_stream::stream;
use axum::extract::State;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::routing::delete;
use axum::{
    Router, extract,
    http::StatusCode,
    response::Html,
    routing::{get, post},
};
use futures_util::Stream;
use log::{error, info};
use minijinja::{Environment, context, path_loader};
use serde::Deserialize;
use std::convert::Infallible;
use tokio::sync::broadcast::Sender as BroadcastSender;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[derive(Clone)]
struct AppState {
    engine: Environment<'static>,
    db: Sender<Command>,
    notifications: BroadcastSender<(Queue, String)>,
}
pub async fn create_webserver(sender: Sender<Command>, broad_send: BroadcastSender<(Queue, String)>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut env = Environment::new();
        env.set_loader(path_loader("templates"));

        let app_state = AppState {
            engine: env,
            db: sender,
            notifications: broad_send,
        };

        let app = Router::new()
            .route("/", get(main_handler))
            .route("/channel/{channel}", get(queue_handler).patch(switch_queue))
            .route("/channel/{channel}/sse", get(sse_handler))
            .route(
                "/channel/{channel}/queue",
                get(queue_fragment)
                    .post(add_team)
                    .patch(move_to_queue)
                    .delete(remove_team),
            )
            .route(
                "/channel/{channel}/queue/{team_num}",
                get(team_fragment).post(add_to_queue),
            )
            .route("/channel/{channel}/queue/{team_num}/{user}", delete(delete_from_queue))
            .route("/create/queue", post(create_queue))
            .with_state(app_state);

        // run it
        let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
            .await
            .expect("Could not bind web server port");
        info!("listening on {}", listener.local_addr().unwrap());
        axum::serve(listener, app).await.unwrap();
    })
}

async fn main_handler() -> Html<&'static str> {
    Html("<h1>Hello, World!</h1>")
}

async fn sse_handler(
    State(state): State<AppState>,
    extract::Path(channel): extract::Path<String>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let mut rx = state.notifications.subscribe();
    let mut env = Environment::new();
    env.set_loader(path_loader("templates"));
    info!("Starting sse");
    Sse::new(stream! {
        while let Ok((queue, e_channel)) = rx.recv().await {
            if e_channel == channel{
                let template = env
                    .get_template("queue.html")
                    .unwrap();
                info!("Sending update");
                let msg = template.render(context! {queue => queue, channel => channel}).unwrap().replace("\n", "").replace("\r", "");
                yield Ok(SseEvent::default().data::<String>(msg))
            }

        }
    })
    .keep_alive(KeepAlive::default())
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
    let (tx, rx) = oneshot::channel();
    let _ = state
        .db
        .send(Command::ShowQueue {
            channel: channel.clone(),
            resp: tx,
        })
        .await;
    let queue = rx.await.unwrap_or_else(|_| Queue::default());
    let template = state
        .engine
        .get_template("queue.html")
        .unwrap()
        .render(context! {queue => queue, channel => channel});
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
            channel: input.channel.clone(),
            resp: tx,
        })
        .await;
    let queue = rx.await.unwrap_or_else(|_| Queue::default());
    let template = state
        .engine
        .get_template("queue.html")
        .unwrap()
        .render(context! {queue => queue, channel => input.channel, active_queue => true});
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
            source: Source::Web,
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
        AddResult::Success(_) => {}
        AddResult::QueueFrozen => {}
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

async fn delete_from_queue(
    state: State<AppState>,
    extract::Path((channel, team_num, user)): extract::Path<(String, usize, String)>,
) -> Result<Html<String>, (StatusCode, Html<String>)> {
    info!("Deleting {} in channel {}", team_num, channel);
    let (tx, rx) = oneshot::channel();
    let _ = state
        .db
        .send(Command::RemoveFromQueue {
            channel: channel.clone(),
            user: user.clone(),
            source: Source::Web,
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
        context! {team => queue.teams[team_num], team_number => team_num, team_size => queue.team_size, channel=> channel},
    );
    Ok(Html(template.unwrap()))
}

#[derive(Deserialize, Debug)]
#[allow(dead_code)]
struct MoveInput {
    user: String,
    team_num: usize,
}
async fn move_to_queue(
    state: State<AppState>,
    extract::Path(channel): extract::Path<String>,
    extract::Form(input): extract::Form<MoveInput>,
) -> Result<Html<String>, (StatusCode, Html<String>)> {
    let team = input.team_num - 1;
    info!("Moving to team {} in channel {}", team, channel);
    let (tx, rx) = oneshot::channel();
    let _ = state
        .db
        .send(Command::MoveToOtherTeam {
            channel: channel.clone(),
            user: input.user.clone(),
            team: team as u8,
            source: Source::Web,
            resp: tx,
        })
        .await;
    let err_template = state.engine.get_template("error.html").unwrap();
    match rx.await.unwrap() {
        MoveResult::Success => {}
        MoveResult::NotFound => {
            let template =
                err_template.render(context! {act => true, msg => "Como lograste mover a alguien que no esta?"});
            return Err((StatusCode::CONFLICT, Html(template.unwrap())));
        }
        MoveResult::NoSpace => {
            let template = err_template.render(context! {act => true, msg => "No hay espacio"});
            return Err((StatusCode::CONFLICT, Html(template.unwrap())));
        }
        MoveResult::InvalidTeam => {
            let template = err_template.render(context! {act => true, msg => "Equipo invalido"});
            return Err((StatusCode::CONFLICT, Html(template.unwrap())));
        }
        MoveResult::AlreadyInTeam => {
            let template = err_template.render(context! {act => true, msg => "Ya esta en ese equipo"});
            return Err((StatusCode::CONFLICT, Html(template.unwrap())));
        }
        MoveResult::GeneralError => {
            let template = err_template.render(context! {act => true, msg => "Error en el servidor"});
            return Err((StatusCode::INTERNAL_SERVER_ERROR, Html(template.unwrap())));
        }
        MoveResult::QueueFrozen => {}
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

    let template = state
        .engine
        .get_template("queue.html")
        .unwrap()
        .render(context! {queue => queue, channel => channel});
    Ok(Html(template.unwrap()))
}

async fn switch_queue(
    extract::Path(channel): extract::Path<String>,
    State(state): State<AppState>,
) -> Result<Html<String>, (StatusCode, Html<String>)> {
    info!("Switching queue status for channel: {}", channel);
    let (tx, rx) = oneshot::channel();
    let _ = state
        .db
        .send(Command::SwitchQueueStatus {
            channel: channel.to_string(),
            resp: tx,
        })
        .await;

    match rx.await.unwrap() {
        Ok(r) => {
            info!("Returning the opposite which is {r}");
            let template = state
                .engine
                .get_template("freeze.html")
                .unwrap()
                .render(context! {channel => channel, active_queue => r});
            Ok(Html(template.unwrap()))
        }
        Err(e) => {
            error!("Error when switching queue for channel {channel}: {e}");
            let err_template = state.engine.get_template("error.html").unwrap();
            let template = err_template.render(context! {act => true, msg => "Equipo invalido"});
            Err((StatusCode::CONFLICT, Html(template.unwrap())))
        }
    }
}

async fn add_team(
    extract::Path(channel): extract::Path<String>,
    State(state): State<AppState>,
) -> Result<String, (StatusCode, Html<String>)> {
    info!("Add team to channel: {}", channel);
    let _ = state
        .db
        .send(Command::AddTeam {
            channel: channel.to_string(),
        })
        .await;

    Ok("ok".to_string())
}

async fn remove_team(
    extract::Path(channel): extract::Path<String>,
    State(state): State<AppState>,
) -> Result<Html<String>, (StatusCode, Html<String>)> {
    info!("Switching queue status for channel: {}", channel);
    let (tx, rx) = oneshot::channel();
    let _ = state
        .db
        .send(Command::RemoveTeam {
            channel: channel.to_string(),
            resp: tx,
        })
        .await;

    match rx.await.unwrap() {
        TeamDeletionResult::Success => Ok(Html("<div id='res' style='display: none'></div>".to_string())),
        TeamDeletionResult::NotEnoughSpaces => {
            error!("Error when deleting ream from queue for channel {channel}");
            let err_template = state.engine.get_template("error.html").unwrap();
            let template = err_template.render(context! {act => true, msg => "No hay espacio para borrar equipos"});
            Err((StatusCode::CONFLICT, Html(template.unwrap())))
        }
        TeamDeletionResult::GeneralError => {
            error!("Error when deleting ream from queue for channel {channel}");
            let err_template = state.engine.get_template("error.html").unwrap();
            let template = err_template.render(context! {act => true, msg => "Error borrando un equipo"});
            Err((StatusCode::CONFLICT, Html(template.unwrap())))
        }
    }
}
