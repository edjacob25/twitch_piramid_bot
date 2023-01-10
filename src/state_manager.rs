use rocksdb::DB;
use tokio::sync::mpsc::Receiver;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

type Responder<T> = oneshot::Sender<T>;

#[derive(Debug)]
pub enum Command {
    Get {
        key: String,
        resp: Responder<bool>,
    },
    Set {
        key: String,
        val: bool,
        resp: Responder<()>,
    },
}

pub fn create_manager(mut receiver: Receiver<Command>) -> JoinHandle<()> {
    let handle = tokio::spawn(async move {
        println!("Starting manager");
        while let Some(cmd) = receiver.recv().await {
            use Command::*;
            let db = DB::open_default("online.db").unwrap();
            match cmd {
                Get { key, resp } => {
                    let res = match db.get(key.clone()) {
                        Ok(Some(values)) => {
                            let val = *values.first().expect("No bytes retrieved");
                            val > 0
                        }
                        Ok(None) => false,
                        Err(_) => false,
                    };
                    println!("Channel {} online status: {}", key, res);
                    let _ = resp.send(res);
                }
                Set { key, val, resp } => {
                    let savable = if val { vec![1] } else { vec![1] };
                    db.put(key, savable).expect("Cannot set online status");
                    resp.send(()).expect("Cannot callback");
                }
            }
        }
    });
    handle
}
