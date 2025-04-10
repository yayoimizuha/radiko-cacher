use std::collections::HashMap;
use std::ops::Deref;
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use reqwest::{Client, Url};
use xml5ever::driver::{parse_document, XmlParseOpts};
use xml5ever::tendril::*;
use anyhow::{Result, Context};
use chrono::{Duration, NaiveDate, Local, DateTime, TimeDelta, Utc};
use std::{env, fmt, process};
use std::fmt::Formatter;
use std::path::PathBuf;
use std::str::FromStr;
use kdam::tqdm;
use unicode_normalization::UnicodeNormalization;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use tokio::join;
use tokio_stream::StreamExt;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RadioChannel {
    id: String,
    name: String,
    banner_url: String,
    area_id: String,
}
impl RadioChannel {
    fn from_hashmap(hash_map: HashMap<&str, String>) -> Result<Self> {
        Ok(RadioChannel {
            id: hash_map.get("id").context("id not found.")?.clone(),
            name: hash_map.get("name").context("name not found.")?.clone().nfkc().collect::<_>(),
            banner_url: hash_map.get("banner").context("banner not found.")?.clone(),
            area_id: hash_map.get("area_id").context("area_id not found.")?.clone(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RadioProgram {
    radio_channel: RadioChannel,
    id: u64,
    // #[serde(with = "firestore::serialize_as_timestamp")]
    ft: String,
    // #[serde(with = "firestore::serialize_as_timestamp")]
    // to: DateTime<Utc>,
    #[serde(serialize_with = "serialize_td", deserialize_with = "deserialize_td")]
    dur: TimeDelta,
    title: String,
    img: Option<String>,
    info: Option<String>,
    desc: Option<String>,
    pfm: Option<String>,
    // on_air_music: Vec<OnAirMusic>,
    #[serde(with = "firestore::serialize_as_timestamp")]
    expire_at: DateTime<Utc>,
}

// pub fn serialize_dt<S>(datetime: &DateTime<Local>, serializer: S) -> Result<S::Ok, S::Error>
// where
//     S: Serializer,
// {
//     let s = datetime.to_rfc3339(); // RFC 3339形式で文字列に変換
//     serializer.serialize_str(&s) // 文字列としてシリアル化
// }
//
// // デシリアライザ
// pub fn deserialize_dt<'de, D>(deserializer: D) -> Result<DateTime<Local>, D::Error>
// where
//     D: Deserializer<'de>,
// {
//     DateTime::parse_from_rfc3339(&String::deserialize(deserializer)?)
//         .map(|dt| dt.with_timezone(&Local))
//         .map_err(serde::de::Error::custom)
// }
//
pub fn serialize_td<S>(timedelta: &TimeDelta, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_i64(timedelta.num_seconds())
}

pub fn deserialize_td<'de, D>(deserializer: D) -> Result<TimeDelta, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(TimeDelta::seconds(i64::deserialize(deserializer)?))
}

impl RadioProgram {
    fn from_hashmap(hash_map: HashMap<String, Option<String>>, radio_channel: RadioChannel) -> Result<Self> {
        Ok(RadioProgram {
            radio_channel,
            id: hash_map.get("id").context("id not found.")?.clone().unwrap().parse::<u64>()?,
            ft: hash_map.get("ft").context("ft not found.")?.clone().unwrap(),
            dur: TimeDelta::seconds(hash_map.get("dur").context("dur not found.")?.clone().unwrap().parse::<i64>()?),
            title: hash_map.get("title").context("title not found.")?.clone().unwrap().nfkc().collect::<_>(),
            img: hash_map.get("img").context("img not found.")?.clone(),
            info: hash_map.get("info").context("info not found.")?.clone().and_then(|s| {
                let body = format!("<body>{s}</body>");
                let dom = parse_document(RcDom::default(), Default::default()).from_utf8().read_from(&mut body.as_bytes()).unwrap();
                let result = node_to_markdown(&dom.document);
                Some(result.nfkc().collect::<_>())
            }),
            desc: hash_map.get("desc").context("desc not found.")?.clone().and_then(|s| {
                let body = format!("<body>{s}</body>");
                let dom = parse_document(RcDom::default(), Default::default()).from_utf8().read_from(&mut body.as_bytes()).unwrap();
                let result = node_to_markdown(&dom.document);
                Some(result.nfkc().collect::<_>())
            }),
            pfm: hash_map.get("pfm").context("pfm not found.")?.clone().and_then(|s| Some(s.nfkc().collect::<_>())),
            expire_at: DateTime::from(DateTime::parse_from_str((hash_map.get("to").context("to not found.").unwrap().clone().unwrap() + " +0900").as_str(), "%Y%m%d%H%M%S %z")?) + TimeDelta::weeks(2),
        })
    }
}
impl fmt::Display for RadioProgram {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "RadioProgram(RadioChannel({}, {}, https://...., {}), {}, {}, {}, {},info: {}, desc: ..., {})", self.radio_channel.id, self.radio_channel.name, self.radio_channel.area_id
               , self.id, self.ft, self.dur.num_minutes(),
               self.info.clone().unwrap_or_else(|| "None".to_owned()), self.title,
               self.pfm.clone().unwrap_or_else(|| "None".to_owned()))
    }
}


fn dig_xml<T>(handle: Handle, path: Vec<&str>, call_func: fn(Handle) -> Option<T>) -> Vec<T> {
    if path.is_empty() {
        return match call_func(handle) {
            None => { vec![] }
            Some(v) => { vec![v] }
        };
    }
    handle.children.borrow().iter().map(|child| {
        match &child.data {
            NodeData::Element { name, .. } => {
                if path[0] == name.local.deref() {
                    dig_xml(child.clone(), path[1..].to_owned(), call_func)
                } else { vec![] }
            }
            _ => vec![]
        }
    }).flatten().collect::<Vec<_>>()
}

fn get_below_string(handle: Handle) -> Option<String> {
    match &handle.children.borrow().clone().get(0) {
        None => None,
        Some(h) => {
            match &h.data {
                NodeData::Text { contents, .. } => { Some(contents.borrow().clone().to_string()) }
                _ => None
            }
        }
    }
}


fn node_to_markdown(handle: &Handle) -> String {
    let dig = |handle: Handle| { handle.children.borrow().clone().into_iter().map(|child| node_to_markdown(&child)).collect::<Vec<_>>().join("") };
    match &handle.data {
        NodeData::Document => {
            dig(handle.clone())
        }
        NodeData::Text { contents } => {
            contents.borrow().to_string()
        }
        NodeData::Element { name, attrs, .. } => {
            match name.local.to_lowercase().as_str() {
                "a" => {
                    let mut href = None;
                    for attr in &attrs.borrow().clone() {
                        if attr.name.local.deref() == "href" {
                            href = Some(attr.value.clone());
                            break;
                        }
                    }
                    match href {
                        None => { dig(handle.clone()) }
                        Some(href) => { format!("[{}]({href})", dig(handle.clone())) }
                    }
                }
                "b" | "strong" => format!("**{}**", dig(handle.clone())),
                "p" => format!("{}\n", dig(handle.clone())),
                "br" => format!("\n\n{}", dig(handle.clone())),
                _ => dig(handle.clone()),
            }
        }
        NodeData::Comment { .. } => String::new(),
        elm => {
            println!("err!:{:?}", elm);
            String::new()
        }
    }
}

#[tokio::main]
async fn main() {
    let client = Client::new();

    let doc = parse_document(RcDom::default(), XmlParseOpts::default()).from_utf8().read_from(
        &mut client.get("https://radiko.jp/v3/station/region/full.xml").send().await.unwrap().text().await.unwrap().as_bytes()
    ).unwrap();
    let channels_hashmap = dig_xml(doc.document, vec!["region", "stations", "station"], |handle| {
        match &handle.data {
            NodeData::Element { .. } => {
                Some(handle.children.borrow().clone().into_iter().filter_map(|child| {
                    match &child.data {
                        NodeData::Element { name, .. } => {
                            match name.local.deref() {
                                "id" => { Some(("id", get_below_string(child).unwrap())) }
                                "name" => { Some(("name", get_below_string(child).unwrap())) }
                                "banner" => { Some(("banner", get_below_string(child).unwrap())) }
                                "area_id" => { Some(("area_id", get_below_string(child).unwrap())) }
                                _ => None
                            }
                        }
                        _ => None
                    }
                }).collect::<HashMap<_, _>>())
            }
            _ => None
        }
    });
    let channels = channels_hashmap.into_iter().map(|hash_map| RadioChannel::from_hashmap(hash_map).unwrap()).collect::<Vec<_>>();
    // for channel in &channels {
    //     println!("{:?}", channel)
    // }

    let program_joiner = channels.iter().map(|channel| NaiveDate::from((Local::now() - Duration::days(1)).naive_local()).iter_days().take(1).map(|date| {
        (channel.clone(), client.get(format!("https://radiko.jp/v3/program/station/date/{}/{}.xml", date.format("%Y%m%d"), channel.id)).send())
    })).flatten().collect::<Vec<_>>();

    let mut programs = vec![];
    for (channel, req) in tqdm!(program_joiner.into_iter(),desc="Parse XML") {
        // if channel.id != "JORF" { continue; }
        let doc = parse_document(RcDom::default(), XmlParseOpts::default()).from_utf8().read_from(&mut req.await.unwrap().text().await.unwrap().as_bytes()).unwrap();
        let programs_hashmaps = dig_xml(doc.document, vec!["radiko", "stations", "station", "progs", "prog"], |handle| match &handle.data {
            NodeData::Element { attrs, .. } => {
                let mut program_meta_hashmap = handle.children.borrow().clone().into_iter().filter_map(|child| {
                    match &child.data {
                        NodeData::Element { name, .. } => {
                            match name.local.deref() {
                                "title" => { Some(("title".to_owned(), get_below_string(child))) }
                                "img" => { Some(("img".to_owned(), get_below_string(child))) }
                                "info" => { Some(("info".to_owned(), get_below_string(child))) }
                                "desc" => { Some(("desc".to_owned(), get_below_string(child))) }
                                "pfm" => { Some(("pfm".to_owned(), get_below_string(child))) }
                                _ => None
                            }
                        }
                        _ => None
                    }
                }).collect::<HashMap<_, _>>();
                let program_date_hashmap = attrs.borrow().clone().into_iter().map(|v| (v.name.local.to_string(), Some(v.value.to_string()))).collect::<HashMap<_, _>>();
                program_meta_hashmap.extend(program_date_hashmap);
                Some(program_meta_hashmap)
            }
            _ => None
        });
        programs.extend(programs_hashmaps.into_iter().filter_map(|hash_map: HashMap<_, _>| {
            match RadioProgram::from_hashmap(hash_map, channel.clone()) {
                Ok(v) => {
                    Some(v)
                }
                Err(_) => { None }
            }
        }).collect::<Vec<_>>());
    }


    let member_json: Value = serde_json::from_str(include_str!("../../src/members.json").nfkc().collect::<String>().as_str()).unwrap();
    // let firestore_db = FirestoreDb::with_options_service_account_key_file(FirestoreDbOptions::new("hello-radiko".to_owned()), PathBuf::from(env::var("FIRESTORE_CRED_JSON").unwrap())).await.unwrap();

    for program in programs {
        let res = search_artist(program.clone(), member_json.clone());
        if !res.is_empty() {
            let prog = program.clone();
            println!("{},{}:{:?}", prog.title.clone(), prog.pfm.clone().unwrap_or("".to_owned()), res);
            println!("{}", serde_json::to_string(&program.clone()).unwrap());
            process::Command::new("yt-dlp").args(vec!["--no-progress".to_owned(), format!("https://radiko.jp/#!/ts/{}/{}", prog.radio_channel.id, prog.ft)]).spawn().unwrap().wait().unwrap();
        }
    }
}


fn search_artist(radio_program: RadioProgram, member_json: Value) -> Vec<String> {
    let mut found = vec![];
    let _ = member_json.as_object().unwrap().into_iter().map(|(group_name, members)| {
        // println!("{group_name}:{members}");
        if group_name != "OG" {
            if radio_program.title.contains(group_name)
                || radio_program.desc.clone().map(|t| { t.contains(group_name) }).unwrap_or(false)
                || radio_program.info.clone().map(|t| { t.contains(group_name) }).unwrap_or(false)
                || radio_program.pfm.clone().map(|t| { t.contains(group_name) }).unwrap_or(false) {
                found.push(group_name.to_owned())
            }
        }
        let _ = members.as_object().unwrap().into_iter().map(|(member_name, literals)| {
            for literal in literals.as_array().unwrap() {
                let literal_string = literal.as_str().unwrap();
                // println!("literal_string:{}", literal_string);
                if radio_program.title.contains(literal_string)
                    || radio_program.desc.clone().map(|t| { t.contains(literal_string) }).unwrap_or(false)
                    || radio_program.info.clone().map(|t| { t.contains(literal_string) }).unwrap_or(false)
                    || radio_program.pfm.clone().map(|t| { t.contains(literal_string) }).unwrap_or(false) {
                    if member_name == "高橋愛" {
                        if radio_program.title.contains("高橋愛子")
                            || radio_program.desc.clone().map(|t| { t.contains("高橋愛子") }).unwrap_or(false)
                            || radio_program.info.clone().map(|t| { t.contains("高橋愛子") }).unwrap_or(false)
                            || radio_program.pfm.clone().map(|t| { t.contains("高橋愛子") }).unwrap_or(false) {
                            break;
                        }
                    }
                    found.push(member_name.to_owned());
                    break;
                }
            }
        }).collect::<Vec<_>>();
    }).collect::<Vec<_>>();
    found
}