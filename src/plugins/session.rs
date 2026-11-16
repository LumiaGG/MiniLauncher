use crate::{config, plugin_catalog::Plugin, plugin_rpc::MAX_FRAME};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::windows::{io::AsRawHandle, process::CommandExt},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{JobObjects::*, Threading::CREATE_NO_WINDOW},
};

struct Job(HANDLE);
// A Job handle is owned, has no thread affinity, and is never used concurrently.
// Moving a session transfers ownership of the handle to its query worker.
unsafe impl Send for Job {}
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
type Outgoing = (Vec<u8>, SyncSender<Result<(), String>>);
pub(super) struct Session {
    child: Child,
    _job: Job,
    writer: SyncSender<Outgoing>,
    reader: Receiver<Result<Value, String>>,
    pub(super) next_id: u64,
    pub(super) last_used: Instant,
    pub(super) idle_secs: u64,
    pub(super) resident: bool,
    pub(super) abandoned_until: Option<Instant>,
}
pub(super) enum RpcError {
    Cancelled,
    Failed(String),
}
impl RpcError {
    pub(super) fn message(self) -> String {
        match self {
            Self::Cancelled => "查询已取消".into(),
            Self::Failed(error) => error,
        }
    }
}
impl Session {
    pub(super) fn start(plugin: &Plugin) -> Result<Self, String> {
        Self::start_cancellable(plugin, None)
    }
    pub(super) fn start_cancellable(
        plugin: &Plugin,
        cancel: Option<(&AtomicU64, u64)>,
    ) -> Result<Self, String> {
        let _timing = crate::diagnostics::timing("plugin_start", &plugin.manifest.id);
        let executable = plugin.directory.join(&plugin.manifest.executable);
        let mut child = Command::new(&executable)
            .args(&plugin.manifest.args)
            .current_dir(&plugin.directory)
            .env(crate::storage::DATA_DIR_ENV, config::data_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("启动 {} 失败：{e}", executable.display()))?;
        let job = unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                let _ = child.kill();
                let _ = child.wait();
                return Err("无法创建插件进程作业".into());
            }
            let job = Job(handle);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(handle, child.as_raw_handle()) == 0
            {
                let _ = child.kill();
                let _ = child.wait();
                return Err("无法隔离插件进程".into());
            }
            job
        };
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (writer, outgoing) = mpsc::sync_channel::<Outgoing>(4);
        std::thread::spawn(move || {
            while let Ok((bytes, reply)) = outgoing.recv() {
                let result = stdin
                    .write_all(&bytes)
                    .and_then(|_| stdin.flush())
                    .map_err(|e| e.to_string());
                let failed = result.is_err();
                let _ = reply.send(result);
                if failed {
                    break;
                }
            }
        });
        let (incoming, reader) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut bytes = Vec::new();
            loop {
                bytes.clear();
                let result = (&mut reader)
                    .take(MAX_FRAME as u64 + 1)
                    .read_until(b'\n', &mut bytes);
                let value = match result {
                    Ok(0) => Err("插件已退出".into()),
                    Ok(_) if bytes.len() > MAX_FRAME || bytes.last() != Some(&b'\n') => {
                        Err("插件响应超过 256 KiB 或缺少换行".into())
                    }
                    Ok(_) => {
                        serde_json::from_slice(&bytes).map_err(|e| format!("无效插件 JSON：{e}"))
                    }
                    Err(error) => Err(error.to_string()),
                };
                let failed = value.is_err();
                if incoming.send(value).is_err() || failed {
                    break;
                }
            }
        });
        let mut session = Self {
            child,
            _job: job,
            writer,
            reader,
            next_id: 0,
            last_used: Instant::now(),
            idle_secs: plugin.manifest.idle_timeout_secs,
            resident: plugin.manifest.resident,
            abandoned_until: None,
        };
        let initialized = session
            .rpc(
                "initialize",
                json!({"protocol_version":2,"host":"MiniLauncher","max_frame_bytes":MAX_FRAME,"settings_path":plugin.settings_path,"config_key":plugin.manifest.config_key.clone().unwrap_or_else(|| crate::plugin_config::key(&plugin.manifest.id))}),
                cancel,
                Duration::from_millis(plugin.manifest.timeout_ms.max(1000)),
            )
            .map_err(RpcError::message)?;
        if initialized.get("protocol_version").and_then(Value::as_u64) != Some(2) {
            return Err("插件未协商 protocol_version = 2".into());
        }
        Ok(session)
    }
    pub(super) fn poll_abandoned(&mut self) -> bool {
        if self.abandoned_until.is_none() {
            return true;
        }
        loop {
            match self.reader.try_recv() {
                Ok(Ok(value)) => {
                    if value.get("id").and_then(Value::as_u64) == Some(self.next_id) {
                        self.abandoned_until = None;
                        return true;
                    }
                }
                Ok(Err(_)) | Err(mpsc::TryRecvError::Disconnected) => return false,
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        self.abandoned_until
            .is_none_or(|deadline| deadline > Instant::now())
    }
    fn write(&mut self, value: Value, timeout: Duration) -> Result<(), RpcError> {
        let mut bytes = serde_json::to_vec(&value).map_err(|e| RpcError::Failed(e.to_string()))?;
        bytes.push(b'\n');
        if bytes.len() > MAX_FRAME {
            return Err(RpcError::Failed("插件请求超过长度限制".into()));
        }
        let (reply, answer) = mpsc::sync_channel(1);
        self.writer
            .try_send((bytes, reply))
            .map_err(|_| RpcError::Failed("插件输入管道没有响应".into()))?;
        answer
            .recv_timeout(timeout)
            .map_err(|_| RpcError::Failed("写入插件超时".into()))?
            .map_err(RpcError::Failed)
    }
    pub(super) fn rpc(
        &mut self,
        method: &str,
        params: Value,
        cancel: Option<(&AtomicU64, u64)>,
        timeout: Duration,
    ) -> Result<Value, RpcError> {
        let _timing = crate::diagnostics::timing("plugin_rpc", method);
        self.last_used = Instant::now();
        self.abandoned_until = None;
        self.next_id += 1;
        let id = self.next_id;
        let deadline = Instant::now() + timeout;
        self.write(
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            timeout,
        )?;
        loop {
            if cancel
                .is_some_and(|(latest, generation)| latest.load(Ordering::Relaxed) != generation)
            {
                self.write(
                    json!({"jsonrpc":"2.0","method":"cancel","params":{"id":id}}),
                    Duration::from_millis(100),
                )?;
                self.abandoned_until = Some(deadline);
                return Err(RpcError::Cancelled);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(RpcError::Failed("插件查询超时".into()));
            }
            match self
                .reader
                .recv_timeout(left.min(Duration::from_millis(20)))
            {
                Ok(Ok(mut value)) => {
                    if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                        return Err(RpcError::Failed("插件没有返回 JSON-RPC 2.0".into()));
                    }
                    if value.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    if let Some(error) = value.get("error") {
                        return Err(RpcError::Failed(format!(
                            "插件错误：{}",
                            error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("未知错误")
                        )));
                    }
                    return value
                        .as_object_mut()
                        .and_then(|object| object.remove("result"))
                        .ok_or_else(|| RpcError::Failed("插件响应缺少 result".into()));
                }
                Ok(Err(error)) => return Err(RpcError::Failed(error)),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(RpcError::Failed("插件输出已关闭".into()))
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            TerminateJobObject(self._job.0, 0);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
