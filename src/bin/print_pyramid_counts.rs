use config::Config;
use rocksdb::{IteratorMode, Options, DB};
extern crate twitch_piramid_bot;

#[allow(dead_code)]
fn check_all_column_names() {
    let settings = Config::builder()
        .add_source(config::File::with_name("data/settings.toml"))
        .build()
        .expect("Need the config");

    let conf = settings
        .try_deserialize::<twitch_piramid_bot::bot_config::BotConfig>()
        .expect("Malformed config");

    let names = conf.channels.iter().map(|x| &*x.channel_name).collect::<Vec<_>>();
    let mut options = Options::default();
    options.create_missing_column_families(true);
    let db = DB::open_cf(&options, "pyramids_test", names).unwrap();
    let cfs = DB::list_cf(&options, "pyramids_test").unwrap_or(vec![]);

    for cf in cfs {
        let handle = db.cf_handle(&*cf).unwrap();
        for item in db.iterator_cf(handle, IteratorMode::Start) {
            let (key, value) = item.unwrap();
            let key = String::from_utf8(key.into_vec()).unwrap();
            let value = String::from_utf8(value.into_vec()).unwrap();
            println!("{} has {}", key, value);
        }
    }
}

fn main() {
    let db = DB::open_default("data/pyramids.db").unwrap();
    let mut last = String::new();
    for item in db.iterator(IteratorMode::Start) {
        let (key, value) = item.unwrap();
        let key = String::from_utf8(key.into_vec()).unwrap();
        let mut parts = key.split(" ");
        let channel = parts.next().unwrap();
        if last != channel {
            println!("\nl{}\n----------------------------------------", channel);
            last = channel.to_string();
        }
        let value = String::from_utf8(value.into_vec()).unwrap();
        println!("{} has {}", parts.next().unwrap(), value);
    }
}
