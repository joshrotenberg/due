use super::{
    Stop,
    config::{Access, Config, Metadata},
    process::{self, Collected, Launch},
    schema,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};
use tokio::{
    sync::{Notify, Semaphore},
    task::JoinHandle,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    role: String,
    workspace: String,
    input: Value,
    prompt: String,
}
struct Lease {
    path: PathBuf,
    access: Access,
    id: usize,
}
#[derive(Default)]
struct State {
    steps: Vec<Option<Value>>,
    leases: Vec<Lease>,
    completed: usize,
}
#[derive(Clone)]
pub(super) struct Host {
    pub config: Arc<Config>,
    pub metadata: Rc<RefCell<BTreeMap<String, Metadata>>>,
    pub stop: Stop,
    dir: Option<PathBuf>,
    state: Arc<Mutex<State>>,
    tasks: Rc<RefCell<Vec<JoinHandle<()>>>>,
    ready: Arc<Notify>,
    permits: Arc<Semaphore>,
}
impl Host {
    pub fn new(config: Config, dir: Option<PathBuf>, stop: Stop) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(config.max_active)),
            config: Arc::new(config),
            metadata: Default::default(),
            stop,
            dir,
            state: Default::default(),
            tasks: Default::default(),
            ready: Default::default(),
        }
    }
    pub fn validate(&self, role: &str, direction: &str, value: &Value) -> Result<(), String> {
        let role = self
            .config
            .roles
            .get(role)
            .ok_or("unknown caller-granted role")?;
        let metas = self.metadata.borrow();
        let meta = metas
            .get(&role.module)
            .ok_or("role module has no metadata")?;
        let schema = match direction {
            "input" => &meta.input_schema,
            "output" => meta
                .answer_schema
                .as_ref()
                .ok_or("role module needs answer_schema")?,
            _ => return Err("validate direction must be input or output".into()),
        };
        super::schema::validate(&super::schema::validator(schema)?, value)
    }
    pub async fn run(&self, value: Value) -> Result<Value, String> {
        let id = self.admit(value).map_err(|e| self.stop.fail(e))?;
        loop {
            let ready = self.ready.notified();
            if let Some(value) = self.state.lock().unwrap().steps[id].clone() {
                return Ok(value);
            }
            ready.await;
        }
    }
    fn admit(&self, value: Value) -> Result<usize, String> {
        if let Some(e) = self.stop.error() {
            return Err(e);
        }
        let req: Request =
            serde_json::from_value(value).map_err(|e| format!("job request: {e}"))?;
        if req.prompt.trim().is_empty() || req.prompt.len() > 128 * 1024 {
            return Err("prompt must be nonempty and <=128 KiB".into());
        }
        let role = self
            .config
            .roles
            .get(&req.role)
            .ok_or("unknown caller-granted role")?;
        if !role.workspaces.contains(&req.workspace) {
            return Err("workspace is not granted to role".into());
        }
        let cwd = self.config.workspaces[&req.workspace]
            .canonicalize()
            .map_err(|e| format!("workspace: {e}"))?;
        if cwd != self.config.workspaces[&req.workspace] {
            return Err("workspace path changed since caller grant".into());
        }
        if !cwd.is_dir() {
            return Err("workspace must be a directory".into());
        }
        let meta = self
            .metadata
            .borrow()
            .get(&role.module)
            .cloned()
            .ok_or("role module has no metadata")?;
        schema::validate(&schema::validator(&meta.input_schema)?, &req.input)
            .map_err(|e| format!("agent input: {e}"))?;
        let (profile_name, profile) = self.config.select(role, &meta)?;
        let binary = self
            .config
            .due_binary
            .as_ref()
            .ok_or("no exact Due binary configured")?
            .canonicalize()
            .map_err(|e| format!("Due binary: {e}"))?;
        if Some(&binary) != self.config.due_binary.as_ref() {
            return Err("Due binary path changed since configuration".into());
        }
        let dir = self
            .dir
            .as_ref()
            .ok_or("effects unavailable during inspection")?;
        let mut state = self.state.lock().unwrap();
        let id = state.steps.len();
        if id >= self.config.max_jobs {
            return Err("global total job cap exceeded".into());
        }
        if state.leases.iter().any(|l| {
            (cwd.starts_with(&l.path) || l.path.starts_with(&cwd))
                && (l.access == Access::Write || role.access == Access::Write)
        }) {
            return Err("overlapping workspace reader/writer grants are already in use".into());
        }
        let dir = dir.join(format!("job-{id:03}"));
        process::directory(&dir)?;
        process::save(
            &dir.join("output.schema.json"),
            meta.answer_schema
                .as_ref()
                .ok_or("role module needs answer_schema")?,
        )?;
        let request = json!({"schema_version":1,"provider":profile.provider,"model":profile.model,"effort":profile.effort,"cwd":cwd,"prompt":req.prompt,"schema":dir.join("output.schema.json"),"ephemeral":true,"native_args":profile.native_args});
        let admitted = json!({"id":id,"role":req.role,"module":role.module,"workspace":req.workspace,"cwd":cwd,"access":role.access,"selected_profile":profile_name,"selected_settings":profile,"due_binary":binary,"input":req.input});
        process::save(&dir.join("admitted.json"), &admitted)?;
        state.steps.push(None);
        state.leases.push(Lease {
            path: cwd,
            access: role.access,
            id,
        });
        drop(state);
        let state = self.state.clone();
        let ready = self.ready.clone();
        let permits = self.permits.clone();
        let stop = self.stop.clone();
        self.tasks.borrow_mut().push(tokio::spawn(async move {
            // Permit held across capture and receipt, shared by all nested groups.
            let permit = permits.acquire_owned().await.expect("semaphore never closed");
            let collected = process::run(Launch { binary, dir: dir.clone(), request }, stop.clone()).await;
            let mut step = outcome(id, &meta, &collected);
            step["admission"] = admitted;
            step["artifacts"] = json!({"directory":dir,"result":dir.join("result.json"),"trace":dir.join("trace.jsonl"),"stderr":dir.join("stderr.txt")});
            {
                let mut state = state.lock().unwrap();
                step["completion_seq"] = json!(state.completed);
                state.completed += 1;
                state.steps[id] = Some(step.clone());
                state.leases.retain(|l| l.id != id);
            }
            let mut receipt = step;
            receipt["child_result"] = collected.result.unwrap_or(Value::Null);
            if let Err(e) = process::save(&dir.join("receipt.json"), &receipt) { stop.fail(e); }
            drop(permit);
            ready.notify_waiters();
        }));
        Ok(id)
    }
    pub async fn finish(&self) {
        let tasks = std::mem::take(&mut *self.tasks.borrow_mut());
        for task in tasks {
            if let Err(e) = task.await {
                self.stop.fail(format!("job supervisor: {e}"));
            }
        }
    }
    pub fn steps(&self) -> Vec<Value> {
        self.state
            .lock()
            .unwrap()
            .steps
            .iter()
            .flatten()
            .cloned()
            .collect()
    }
}

fn outcome(id: usize, meta: &Metadata, c: &Collected) -> Value {
    let raw = c.result.as_ref();
    let protocol = raw
        .ok_or_else(|| "no JSON result".to_string())
        .and_then(|r| {
            let contract: Value = serde_json::from_str(include_str!("due-result.schema.json"))
                .map_err(|e| e.to_string())?;
            schema::validate(&schema::validator(&contract)?, r)?;
            if (r["status"] == "succeeded") != r["error"].is_null()
                || r["structured_output_present"] == false && !r["structured_output"].is_null()
            {
                return Err("inconsistent Due status/error or output presence".into());
            }
            Ok(())
        });
    let present = raw.is_some_and(|r| r["structured_output_present"] == true);
    let validation = raw
        .filter(|_| present)
        .ok_or("structured output missing".into())
        .and_then(|r| {
            crate::data::check(&r["structured_output"]).map_err(|e| e.to_string())?;
            schema::validate(
                &schema::validator(
                    meta.answer_schema
                        .as_ref()
                        .ok_or("role module needs answer_schema")?,
                )?,
                &r["structured_output"],
            )
        });
    let status = if c.cancel_requested
        || c.stopped_before_spawn
        || raw.is_some_and(|r| r["status"] == "cancelled")
    {
        "cancelled"
    } else if c.exit["started"] == false {
        "not_started"
    } else if protocol.is_err() {
        "protocol_error"
    } else if c.exit["code"] != 0
        || raw.is_some_and(|r| r["status"] != "succeeded" || !r["error"].is_null())
    {
        "native_failure"
    } else if !c.errors.is_empty() {
        "protocol_error"
    } else if validation.is_err() {
        "invalid_output"
    } else {
        "succeeded"
    };
    let mut result = json!({"id":id,"status":status,"ok":status == "succeeded","exit":c.exit,"errors":c.errors,"cancel_requested":c.cancel_requested,"answer_present":present,"validation":{"valid":validation.is_ok(),"error":validation.err()},"observed_model":null,"trace_bytes":c.trace_bytes,"trace_summary":raw.and_then(|r| r.get("trace")),"child_error":raw.and_then(|r| r.get("error"))});
    result["protocol"] = json!({"valid":protocol.is_ok(),"error":protocol.err()});
    result["stopped_before_spawn"] = json!(c.stopped_before_spawn);
    if present {
        result["answer"] = raw.unwrap()["structured_output"].clone();
    }
    result
}
