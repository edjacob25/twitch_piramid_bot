use config::Config;
use rocksdb::{IteratorMode, Options, DB};
extern crate twitch_piramid_bot;

fn check_all_column_names() {
    let settings = Config::builder()
        .add_source(config::File::with_name("settings.toml"))
        .build()
        .expect("Need the config");

    let conf = settings
        .try_deserialize::<twitch_piramid_bot::bot_config::BotConfig>()
        .expect("Malformed config");

    let names = conf
        .channels
        .iter()
        .map(|x| &*x.channel_name)
        .collect::<Vec<_>>();
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
    let db = DB::open_default("pyramids").unwrap();
    for item in db.iterator(IteratorMode::Start) {
        let (key, value) = item.unwrap();
        let key = String::from_utf8(key.into_vec()).unwrap();
        let value = String::from_utf8(value.into_vec()).unwrap();
        println!("{} has {}", key, value);
    }
}
