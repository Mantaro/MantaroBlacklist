#![feature(proc_macro_hygiene, decl_macro)]

extern crate kankyo;
extern crate rocket_contrib;
extern crate rocksdb;
extern crate serenity;
extern crate typemap;
#[macro_use] extern crate rocket;
#[macro_use] extern crate serde_derive;

use rocket::State;
use rocket::Outcome;
use rocket::http::Status;
use rocket::request::{self, Request, FromRequest};
use rocket_contrib::json::Json;
use rocksdb::DB;
use serenity::client::Client;
use serenity::prelude::EventHandler;
use serenity::framework::standard::Args;
use serenity::framework::standard::StandardFramework;
use std::env;
use std::sync::Arc;
use std::thread;
use typemap::Key;

struct DBKey;
impl Key for DBKey {
    type Value = Arc<DB>;
}

struct WhitelistKey;
impl Key for WhitelistKey {
    type Value = Vec<u64>;
}

#[derive(Deserialize)]
struct Reason {
    reason: String
}

#[derive(Serialize, Deserialize)]
struct Response {
    reason: String,
    id: u64
}

#[post("/reason/<id>", format = "application/json", data = "<body>")]
fn set_reason(db: State<Arc<DB>>, id: u64, body: Json<Reason>, api_key: ApiKey) -> Json<Response> {
    db.put(&format!("{}", id).as_bytes(), body.0.reason.as_bytes()).expect("unable to save");
    Json(Response {reason: body.0.reason, id: id})
}

#[get("/reason/<id>")]
fn get_reason(db: State<Arc<DB>>, id: u64, api_key: ApiKey) -> Result<Option<Json<Response>>, ()> {
    match db.get(&format!("{}", id).as_bytes()) {
        Ok(Some(value)) => Ok(Some(Json(Response {reason: value.to_utf8().unwrap().to_string(), id: id}))),
        Ok(None) => Ok(None),
        Err(_) => Err(())
    }
}

fn main() {
    kankyo::load().expect("Unable to load .env");
    env::var("KEY").expect("No KEY defined");
    let whitelist = env::var("WHITELIST").expect("No whitelist defined").split(",")
        .map(|s| s.trim().to_string().parse::<u64>().expect("Malformed user id"))
        .collect::<Vec<_>>();
    let db = Arc::new(DB::open_default(env::var("DB_PATH").unwrap_or("data".to_string())).expect("Unable to create rocksdb instance"));

    let mut client = Client::new(&env::var("TOKEN").expect("token"), Handler)
        .expect("Error creating client");
    client.with_framework(StandardFramework::new()
        .configure(|c| c.prefix(&env::var("PREFIX").unwrap_or("~".to_string()))) // set the bot's prefix to "~"
        .on("lookup", |ctx, msg, mut args| {
            if args.is_empty() {
                msg.channel_id.send_message(|m| m.content("Please provide an user id"))?;
                return Ok(());
            }
            let id = match args.single::<u64>() {
                Ok(i) => i,
                Err(_) => {
                    msg.channel_id.send_message(|m| m.content("Invalid user id"))?;
                    return Ok(());
                }
            };
            let data = ctx.data.lock();
            match data.get::<DBKey>().unwrap().get(&format!("{}", id).as_bytes())? {
                Some(value) => msg.channel_id.send_message(|m| m.content(value.to_utf8().unwrap())),
                None => msg.channel_id.send_message(|m| m.content("No reason found"))
            }?;
            Ok(())
        })
        .on("setreason", |ctx, msg, mut args| {
            let data = ctx.data.lock();
            if !data.get::<WhitelistKey>().unwrap().contains(msg.author.id.as_u64()) {
                msg.channel_id.send_message(|m| m.content("You do not have permission to use this command"))?;
                return Ok(());
            }
            if args.is_empty() {
                msg.channel_id.send_message(|m| m.content("Please provide an user id"))?;
                return Ok(());
            }
            let mut copy = Args::new(args.rest(), &[" ".to_string()]);
            let mut ids: Vec<u64> = Vec::new();
            for id in copy.iter::<u64>() {
                if !id.is_ok() {
                    break;
                }
                ids.push(id.unwrap());
            }
            if ids.len() == 0 {
                msg.channel_id.send_message(|m| m.content("Invalid user id(s)"))?;
                return Ok(());
            }
            args.skip_for(ids.len() as u32);
            if args.is_empty() {
                msg.channel_id.send_message(|m| m.content("Please provide an user id"))?;
                return Ok(());
            }
            let reason = args.rest();
            let db = data.get::<DBKey>().unwrap();
            for id in &ids {
                db.put(&format!("{}", id).as_bytes(), reason.as_bytes()).expect("unable to save");
            }
            msg.channel_id.send_message(|m| m.content("Successfully saved"))?;
            Ok(())
        })
    );

    {
        let mut data = client.data.lock();
        data.insert::<DBKey>(Arc::clone(&db));
        data.insert::<WhitelistKey>(whitelist);
    }

    thread::spawn(move || {
        rocket::ignite().mount("/", routes![set_reason, get_reason]).manage(db).launch();
    });

    // start listening for events by starting a single shard
    if let Err(why) = client.start() {
        println!("An error occurred while running the client: {:?}", why);
    }
}

struct Handler;

impl EventHandler for Handler {}

struct ApiKey(String);

#[derive(Debug)]
enum ApiKeyError {
    BadCount,
    Missing,
    Invalid,
}

fn is_valid(key: &str) -> bool {
        key == &env::var("KEY").unwrap()
}

impl<'a, 'r> FromRequest<'a, 'r> for ApiKey {
    type Error = ApiKeyError;

    fn from_request(request: &'a Request<'r>) -> request::Outcome<Self, Self::Error> {
        let keys: Vec<_> = request.headers().get("authorization").collect();
        match keys.len() {
            0 => Outcome::Failure((Status::BadRequest, ApiKeyError::Missing)),                                      
            1 if is_valid(keys[0]) => Outcome::Success(ApiKey(keys[0].to_string())),
            1 => Outcome::Failure((Status::BadRequest, ApiKeyError::Invalid)),
            _ => Outcome::Failure((Status::BadRequest, ApiKeyError::BadCount)),
        }
    }
}
