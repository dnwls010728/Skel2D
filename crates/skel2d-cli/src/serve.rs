//! Local HTTP API over one document. Plain JSON over HTTP so any agent can
//! call it with curl, without loading tool schemas into its context.
//!
//! Endpoints (all bound to 127.0.0.1):
//!   GET  /doc                         full document
//!   GET  /info                        summary
//!   GET  /validate                    issues list
//!   GET  /pose?anim=&time=&physics=&fps=&move=   world-space pose
//!   GET  /render?anim=&time=&frames=&width=&height=&zoom=&bones=0&physics=&fps=&move=   PNG
//!   GET  /schema/ops | /schema/document
//!   POST /ops[?dry_run=1]             body: op or array of ops; saves on success
//!   POST /undo                        revert the last successful /ops
//!
//! Changes made to the file by other tools are picked up before each request.

use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{Value, json};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::{PhysicsArgs, RenderArgs};
use crate::commands::{base_dir, error_json, info_json, pose_json, render_bytes};
use crate::session::Session;

const MAX_BODY: u64 = 8 * 1024 * 1024;

pub fn run(file: &Path, port: u16) -> Result<Value> {
    let mut state = Session::open(file)?;
    let addr = format!("127.0.0.1:{port}");
    let server = Server::http(&addr).map_err(|e| anyhow::anyhow!("cannot listen on {addr}: {e}"))?;
    eprintln!("{}", json!({ "listening": format!("http://{addr}"), "file": file }));

    for mut req in server.incoming_requests() {
        let reply = match handle(&mut state, &mut req) {
            Ok(r) => r,
            Err(e) => Reply::json(400, error_json(&e)),
        };
        let response = Response::from_data(reply.body)
            .with_status_code(reply.status)
            .with_header(Header::from_bytes("Content-Type", reply.content_type).expect("static header"));
        let _ = req.respond(response);
    }
    Ok(Value::Null)
}

struct Reply {
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
}

impl Reply {
    fn json(status: u16, v: Value) -> Self {
        Reply { status, content_type: "application/json", body: skel2d_core::format::to_compact(&v).unwrap_or_default().into_bytes() }
    }

    fn ok(v: Value) -> Self {
        Self::json(200, v)
    }
}

fn handle(state: &mut Session, req: &mut Request) -> Result<Reply> {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let q = Query::parse(query);
    let method = req.method().clone();

    state.reload_if_changed()?;
    let doc = &state.doc;

    Ok(match (method, path) {
        (Method::Get, "/doc") => Reply::ok(serde_json::to_value(doc)?),
        (Method::Get, "/info") => Reply::ok(info_json(doc)),
        (Method::Get, "/validate") => Reply::ok(json!({ "issues": skel2d_core::validate(doc) })),
        (Method::Get, "/pose") => Reply::ok(pose_json(doc, q.get("anim"), q.num("time")?.unwrap_or(0.0), q.num("since")?, q.get("layers"), &q.get("skins").map(|t| t.split(',').map(str::to_string).collect::<Vec<_>>()).unwrap_or_default(), &q.physics()?)?),
        (Method::Get, "/render") => {
            let args = RenderArgs {
                anim: q.get("anim").map(str::to_string),
                time: q.num("time")?.unwrap_or(0.0),
                frames: q.num("frames")?.map(|v: f32| v as u32),
                width: q.num("width")?.map_or(512, |v: f32| v as u32),
                height: q.num("height")?.map_or(512, |v: f32| v as u32),
                zoom: q.num("zoom")?,
                no_bones: q.get("bones") == Some("0"),
                center: q.get("center").and_then(|c| crate::parse_point(c).ok()),
                tracks: q.get("tracks").map(|t| t.split(',').map(str::to_string).collect()).unwrap_or_default(),
                layers: q.get("layers").map(str::to_string),
                skins: q.get("skins").map(|t| t.split(',').map(str::to_string).collect()).unwrap_or_default(),
                physics: q.physics()?,
            };
            let (png, _) = render_bytes(doc, &base_dir(&state.file), &args)?;
            Reply { status: 200, content_type: "image/png", body: png }
        }
        (Method::Get, "/gif") => {
            let job = skel2d_render::GifJob {
                anim: q.get("anim").map(str::to_string),
                layers: skel2d_core::parse_layer_tracks(doc, q.get("layers").unwrap_or("[]"))?,
                skins: q.get("skins").map(|t| t.split(',').map(str::to_string).collect()).unwrap_or_default(),
            };
            let opts: skel2d_render::GifOptions = q.get("options").map(serde_json::from_str).transpose()?.unwrap_or_default();
            let (gif, _) = crate::commands::gif_bytes(doc, &base_dir(&state.file), &job, &opts)?;
            Reply { status: 200, content_type: "image/gif", body: gif }
        }
        (Method::Get, "/schema/ops") => Reply::ok(crate::commands::schema("ops")?),
        (Method::Get, "/schema/document") => Reply::ok(crate::commands::schema("document")?),
        (Method::Get, "/schema/tracks") => Reply::ok(crate::commands::schema("tracks")?),
        (Method::Post, "/ops") => {
            let mut body = String::new();
            req.as_reader().take(MAX_BODY).read_to_string(&mut body).context("reading request body")?;
            let ops = skel2d_rig::parse_ops(&body)?;
            let report = skel2d_rig::apply_any(doc, &base_dir(&state.file), &ops)?;
            let dry_run = matches!(q.get("dry_run"), Some("1" | "true"));
            if !dry_run {
                state.commit(report.document)?;
            }
            Reply::ok(json!({ "applied": ops.len(), "expanded":report.ops.len(), "meshes":report.meshes, "pruned_influences":report.pruned_influences, "saved": !dry_run }))
        }
        (Method::Post, "/undo") => match state.undo()? {
            Some(remaining) => Reply::ok(json!({ "undone": true, "remaining": remaining })),
            None => Reply::json(409, json!({ "error": "nothing to undo" })),
        },
        (_, p) => Reply::json(404, json!({ "error": format!("no route {p}") })),
    })
}

struct Query(Vec<(String, String)>);

impl Query {
    fn parse(s: &str) -> Self {
        Query(
            s.split('&')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let (k, v) = p.split_once('=').unwrap_or((p, ""));
                    (percent_decode(k), percent_decode(v))
                })
                .collect(),
        )
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn physics(&self) -> Result<PhysicsArgs> {
        Ok(PhysicsArgs { physics: self.get("physics").map(str::to_string), fps: self.num("fps")?, moves: self.get("move").map(str::to_string) })
    }

    fn num(&self, key: &str) -> Result<Option<f32>> {
        self.get(key)
            .map(|v| v.parse::<f32>().with_context(|| format!("query {key}={v:?} is not a number")))
            .transpose()
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(hi), Some(lo)) => {
                    out.push(hi << 4 | lo);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
