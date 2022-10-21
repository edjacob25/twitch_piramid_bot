use rocksdb::{DB,IteratorMode};


fn main() {
    let db = DB::open_default("pyramids").unwrap();
    for item in db.iterator(IteratorMode::Start) {
        let (key, value) = item.unwrap();
        let key = String::from_utf8(key.into_vec()).unwrap();
        let value = String::from_utf8(value.into_vec()).unwrap();
        println!("{} has {}", key, value);
    }
}