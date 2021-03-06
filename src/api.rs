use app::App;
use std::sync::Arc;
use std::thread;
use rocket;
use rocket::*;
use rocket::http::*;
use rocket_contrib::Json;
use serde_json::*;

#[get("/poolstats")]
fn poolstats(app: State<Arc<App>>) -> Json<Value> {
  let hashrates = app.db.get_hashrates();
  Json(json!({
    "total_fee": app.total_fee(),
    "blocks": app.db.all_blocks(),
    "hashrates": hashrates,
  }))
}

#[get("/minerstats/<address>")]
fn minerstats(app: State<Arc<App>>, address: &RawStr) -> Json<Value> {
  let address = address.as_str();
  let hashrates = app.db.hashrates_by_address(&app.address_pattern, address);
  let transactions = app.db.transactions_by_address(address);
  Json(json!({
    "hashrates": hashrates,
    "transactions": transactions,
  }))
}

pub fn init(app: Arc<App>) {
  thread::spawn(move || {
    rocket::ignite()
      .manage(app)
      .mount("/", routes![poolstats, minerstats]).launch();
  });
}
