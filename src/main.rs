#[macro_use]
extern crate serde_derive;
extern crate toml;
#[macro_use]
extern crate serde_json;
extern crate serde;
extern crate reqwest;


use std::fs::File;
use std::io::prelude::*;
use std::io;
use std::collections::BTreeMap;
use std::cmp::Ordering;
use serde_json::Value;
use serde::de::Deserialize;
use serde::de::DeserializeOwned;

#[derive(Deserialize,Clone)]
struct Config {
    api_url: String,
    user: String,
    pass: String,
    maildir: String,
}

#[derive(Deserialize)]
struct TtrssResponse<T> {
    seq: u32,
    status: u32,
    content: T,
}

#[derive(Deserialize, Debug)]
struct Feed {
    title: String,
    feed_url: String,
    id: u32,
    last_updated: u32,
    cat_id: u32,
    order_id: u32,
}

//one could also obtain tags etc here as well
#[derive(Deserialize, Debug, Eq, Ord, PartialOrd, PartialEq)]
struct Headline {
    id: u32,
    unread: bool,
    marked: bool,
    title: String,
    feed_id: u32,
    author: String,
    link: String,
    comments_link: String,
}


#[derive(Deserialize, Debug)]
struct Article {
    id: u32,
    content: String,
}

#[derive(Deserialize, Debug)]
struct Login {
    session_id: String,
}

struct TtrssRequest {
    session_id: Option<String>,
    config: Config,
}

enum TtrssOperation {
    Login,
    GetFeeds,
    GetHeadlines(u32, u32),
    GetArticle(String),
}

impl TtrssRequest {
    fn new_login(config: Config) -> TtrssRequest {
        TtrssRequest {
            session_id: None,
            config: config,
        }
    }

    fn new(login_req: TtrssRequest, session_id: String) -> TtrssRequest {
        TtrssRequest {
            session_id: Some(session_id),
            config: login_req.config,
        }
    }

    fn get_req_json(&self, op: TtrssOperation) -> serde_json::Value {
        match op {
            TtrssOperation::Login => {
                json!({"op":"login",
                       "user":self.config.user,
                       "password":self.config.pass})
            }
            TtrssOperation::GetFeeds => {
                json!({"op":"getFeeds",
                       "sid":self.session_id.clone().unwrap()})
            }
            TtrssOperation::GetHeadlines(feed_id, since_id) => {
                json!({"op":"getHeadlines",
                       "user":self.config.user,
                       "password":self.config.pass,
                       "sid":self.session_id.clone().unwrap(),
                       "feed_id":feed_id,
                       "since_id":since_id})
            }
            TtrssOperation::GetArticle(ids) => {
                json!({"op":"getArticle",
                       "user":self.config.user,
                       "sid":self.session_id.clone().unwrap(),
                       "article_id":ids,
                       "password":self.config.pass})
            }

        }
    }

    fn call<T>(&self, op: TtrssOperation) -> Result<T, SyncError>
        where T: DeserializeOwned
    {
        let req_json = self.get_req_json(op);

        println!("request json: {}", req_json.to_string());

        let client = reqwest::Client::new()?;
        let mut res = client.post(&self.config.api_url)?.body((&req_json.to_string()).to_owned()).send()?;
        let mut content = String::new();
        res.read_to_string(&mut content);

        println!("response json: {}", &content);

        let v: Value = try!(serde_json::from_str(&content));

        //test response first, then Value can be cast per serde_json::from_value
        let status: u64 = v["status"].as_u64().unwrap();
        if status == 0 {
            let response: TtrssResponse<T> = try!(serde_json::from_value(v));
            return Ok(response.content);
        }
        //TODO maybe different error codes depending on status codes
        Err(SyncError::BadStatus)
    }
}

#[derive(Debug)]
enum SyncError {
    RssIo(reqwest::Error),
    FileIo(io::Error),
    BadStatus,
    BadConfig(toml::de::Error),
    BadRssJson(serde_json::Error),
}

//rust makes java error handling look concise
impl From<reqwest::Error> for SyncError {
    fn from(err: reqwest::Error) -> SyncError {
        SyncError::RssIo(err)
    }
}

impl From<io::Error> for SyncError {
    fn from(err: io::Error) -> SyncError {
        SyncError::FileIo(err)
    }
}

impl From<toml::de::Error> for SyncError {
    fn from(err: toml::de::Error) -> SyncError {
        SyncError::BadConfig(err)
    }
}

impl From<serde_json::Error> for SyncError {
    fn from(err: serde_json::Error) -> SyncError {
        SyncError::BadRssJson(err)
    }
}

//this should ultimately be a poll loop
//it should catch any errors due to not being
//logged in and re-log in, then commence loop from the top
fn main() {
    let config: Config = get_config().unwrap();
    let login_req = TtrssRequest::new_login(config.clone());
    let login_result: Result<Login, SyncError> = login_req.call(TtrssOperation::Login);

    let session_id: String = login_result.unwrap().session_id;

    println!("Got:{}", session_id);

    let req = TtrssRequest::new(login_req, session_id);
    println!("Session id? {:?}", &req.session_id);
    let feeds_result: Result<Vec<Feed>, SyncError> = req.call(TtrssOperation::GetFeeds);
    let feeds = feeds_result.unwrap();
    for feed in &feeds {
        println!("Feed: {:?}", feed);
        let hl_result: Result<Vec<Headline>, SyncError> =
            req.call(TtrssOperation::GetHeadlines(feed.id, 0));
        let hls = hl_result.unwrap();

        let mut items = Vec::new();

        let hl_ids = hls.iter().map(|ref hl| hl.id.to_string()).collect::<Vec<_>>().join(",");

        let ar_result: Result<Vec<Article>, SyncError> =
            req.call(TtrssOperation::GetArticle(hl_ids));
        let ars = ar_result.unwrap();
        let mut i = 0;
        for ar in ars {
            //hl => article should be 1-1 - why is there a loop here??
            items.push((&hls[i], ar));
            //items.insert(&hls[i], ar);
            //println!("Article: {:?}", &ar);
            i = i + 1;
        }

        println!("items: {:?}", &items);
        //write to local fs

        write_maildir(&config, feed, items);
    }
    println!("END");
}

fn write_maildir(config: &Config, feed: &Feed, items: Vec<(&Headline, Article)>) {}

fn get_config() -> Result<Config, SyncError> {
    let mut f = try!(File::open("sync.toml"));
    let mut s = String::new();
    try!(f.read_to_string(&mut s));
    let config: Config = try!(toml::from_str(&s));
    Ok(config)
}
