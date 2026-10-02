//! Session UUID 标准化 — 统一多客户端会话标识
//!
//! 核心功能：
//! 1. 从 headers/body 提取 UUID 候选（支持多来源、优先级排序）
//! 2. 验证并规范化 UUID（去除 session_ 前缀、验证格式）
//! 3. Fallback 到内容派生 + UUID 映射（内存 + JSON 持久化）
//! 4. 错误降级（存储故障 → 临时 UUID）
//!
//! 设计对齐 Python 仓库的 `app/services/session_uuid_service.py`

use axum::http::HeaderMap;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;
use base64::Engine;

/// 默认映射条目上限
#[allow(dead_code)]
const DEFAULT_CAPACITY: usize = 2048;
/// 默认 TTL（30 天）
#[allow(dead_code)]
const DEFAULT_TTL_SECS: i64 = 30 * 24 * 3600;

/// Header 候选优先级列表（按顺序匹配）
const HEADER_CANDIDATES: &[&str] = &[
    "x-session-affinity",
    "x-client-request-id",
    "session_id",
    "x-claude-code-session-id",
];

/// UUID 映射条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UuidEntry {
    /// 映射的 UUID
    #[serde(with = "uuid::serde::compact")]
    pub uuid: Uuid,
    /// 过期时间戳（unix 秒）
    pub expires_at: i64,
    /// 上次访问时间（用于 LRU 淘汰）
    pub last_accessed_at: i64,
}

/// Session UUID 存储（内存 + JSON 持久化）
#[derive(Clone)]
pub struct SessionUuidStore {
    inner: Arc<Mutex<StoreInner>>,
    persist_path: PathBuf,
}

struct StoreInner {
    /// derived_hash → UuidEntry
    mappings: HashMap<String, UuidEntry>,
    capacity: usize,
}

impl SessionUuidStore {
    /// 创建新的存储实例
    pub fn new(persist_path: PathBuf, capacity: usize) -> Self {
        let mut mappings = HashMap::new();

        // 启动时从磁盘加载
        if let Ok(data) = std::fs::read_to_string(&persist_path) {
            if let Ok(loaded) = serde_json::from_str::<HashMap<String, UuidEntry>>(&data) {
                let now = chrono::Utc::now().timestamp();
                // 只保留未过期的条目
                for (key, entry) in loaded {
                    if entry.expires_at > now {
                        mappings.insert(key, entry);
                    }
                }
                tracing::info!(
                    loaded_count = mappings.len(),
                    path = %persist_path.display(),
                    "Loaded session UUID mappings"
                );
            }
        }

        Self {
            inner: Arc::new(Mutex::new(StoreInner { mappings, capacity })),
            persist_path,
        }
    }

    /// 查询映射的 UUID
    pub fn get_uuid(&self, derived_hash: &str) -> Option<Uuid> {
        let mut inner = self.inner.lock();
        let now = chrono::Utc::now().timestamp();

        if let Some(entry) = inner.mappings.get_mut(derived_hash) {
            if entry.expires_at > now {
                entry.last_accessed_at = now;
                return Some(entry.uuid);
            }
            // 过期，移除
            inner.mappings.remove(derived_hash);
        }
        None
    }

    /// 设置 UUID 映射
    pub fn set_uuid(&self, derived_hash: String, uuid: Uuid, ttl_seconds: i64) {
        let mut inner = self.inner.lock();
        let now = chrono::Utc::now().timestamp();

        // LRU 淘汰：如果超过容量，移除最老的条目
        if inner.mappings.len() >= inner.capacity {
            if let Some((oldest_key, _)) = inner
                .mappings
                .iter()
                .min_by_key(|(_, entry)| entry.last_accessed_at)
                .map(|(k, v)| (k.clone(), v.clone()))
            {
                inner.mappings.remove(&oldest_key);
            }
        }

        inner.mappings.insert(
            derived_hash,
            UuidEntry {
                uuid,
                expires_at: now + ttl_seconds,
                last_accessed_at: now,
            },
        );
    }

    /// 持久化到磁盘
    pub fn persist(&self) {
        let inner = self.inner.lock();
        if let Ok(json) = serde_json::to_string_pretty(&inner.mappings) {
            if let Err(e) = std::fs::write(&self.persist_path, json) {
                tracing::warn!(error = %e, "Failed to persist session UUID mappings");
            }
        }
    }
}

/// Session UUID 解析结果
#[derive(Debug, Clone)]
pub struct SessionUuidResult {
    /// 最终 session ID（session_{uuid} 格式），或 None
    pub session_id: Option<String>,
    /// 来源标签（用于可观测性）
    pub source: String,
    /// 原始提取值（用于诊断）
    pub original_value: Option<String>,
}

/// 从 headers/body 提取 UUID 候选值
fn extract_uuid_candidates(
    headers: &HeaderMap,
    prompt_cache_key: Option<&str>,
) -> Vec<(String, String)> {
    let mut candidates = Vec::new();

    // 1. Header 候选（按优先级顺序）
    for key in HEADER_CANDIDATES {
        if let Some(value) = headers.get(*key).and_then(|v| v.to_str().ok()) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                candidates.push((format!("header:{}", key), trimmed.to_string()));
            }
        }
    }

    // 2. Body 候选（prompt_cache_key，支持 Codex CLI）
    if let Some(cache_key) = prompt_cache_key {
        let trimmed = cache_key.trim();
        if !trimmed.is_empty() {
            candidates.push(("body:prompt_cache_key".to_string(), trimmed.to_string()));
        }
    }

    candidates
}

/// 验证并规范化 UUID
fn validate_and_normalize_uuid(candidate: &str) -> Option<Uuid> {
    if candidate.is_empty() {
        return None;
    }

    // 去除 session_ 前缀（如果存在）
    let raw = candidate.strip_prefix("session_").unwrap_or(candidate);

    // SEC-1: 截断到 256 字符避免恶意超长输入
    let raw = if raw.len() > 256 {
        &raw[..256]
    } else {
        raw
    };

    Uuid::parse_str(raw).ok()
}

/// 内容派生：从稳定请求内容生成派生 session ID
///
/// 对齐 Python 的 `derive_content_session_id` 逻辑
pub fn derive_content_session_id(
    secret: &str,
    auth_namespace: &str,
    client_user: &str,
    first_user_text: &str,
    max_text_bytes: usize,
) -> Option<String> {
    // 规范化文本（Unicode NFC + 换行符统一）
    let normalized_text = normalize_first_user_text(first_user_text);
    let encoded_text = normalized_text.as_bytes();

    // 验证输入
    if secret.is_empty()
        || normalized_text.is_empty()
        || max_text_bytes == 0
        || encoded_text.len() > max_text_bytes
        || (auth_namespace.is_empty() && client_user.is_empty())
    {
        return None;
    }

    // 构造 HMAC 材料（与 Python 一致的 JSON 格式）
    let material = serde_json::json!({
        "auth_namespace": auth_namespace,
        "client_user": client_user,
        "first_user_text": normalized_text,
        "purpose": "ai-models-router/session-id",
        "version": 1,
    });

    let material_bytes = serde_json::to_string(&material).ok()?;

    // HMAC-SHA256，取前 16 字节
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;

    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(material_bytes.as_bytes());
    let result = mac.finalize();
    let digest = &result.into_bytes()[..16];

    // Base64 URL-safe 编码（去除 padding）
    let token = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);

    Some(format!("derived:v1:{}", token))
}

/// 规范化首条用户消息文本
fn normalize_first_user_text(text: &str) -> String {
    // Unicode NFC 规范化
    use unicode_normalization::UnicodeNormalization;
    let normalized: String = text.nfc().collect();

    // 统一换行符
    let normalized = normalized.replace("\r\n", "\n").replace('\r', "\n");

    normalized.trim().to_string()
}

/// 将派生 session ID 映射到 UUID
fn derive_to_uuid_fallback(
    derived_session_id: &str,
    uuid_store: &SessionUuidStore,
    ttl_seconds: i64,
) -> Result<(Uuid, String), String> {
    // 生成 hash key（取 SHA256 前 16 字符）
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(derived_session_id.as_bytes());
    let result = hasher.finalize();
    let derived_hash = format!("{:x}", result)[..16].to_string();

    // 1. 查询现有映射
    if let Some(cached_uuid) = uuid_store.get_uuid(&derived_hash) {
        return Ok((cached_uuid, "derived_mapped".to_string()));
    }

    // 2. 生成新 UUID 并存储
    let new_uuid = Uuid::new_v4();
    uuid_store.set_uuid(derived_hash, new_uuid, ttl_seconds);

    Ok((new_uuid, "derived_generated".to_string()))
}

/// 解析 session UUID（主入口）
pub fn resolve_session_uuid(
    headers: &HeaderMap,
    prompt_cache_key: Option<&str>,
    uuid_store: Option<&SessionUuidStore>,
    content_fallback_enabled: bool,
    secret: Option<&str>,
    auth_namespace: Option<&str>,
    client_user: Option<&str>,
    first_user_text: Option<&str>,
    ttl_seconds: i64,
) -> SessionUuidResult {
    // 阶段 1: 尝试从 headers/body 提取 UUID
    let candidates = extract_uuid_candidates(headers, prompt_cache_key);

    for (source, candidate) in candidates {
        if let Some(uuid_obj) = validate_and_normalize_uuid(&candidate) {
            let session_id = format!("session_{}", uuid_obj);
            return SessionUuidResult {
                session_id: Some(session_id),
                source: format!("uuid:{}", source),
                original_value: Some(candidate),
            };
        }
    }

    // 阶段 2: Fallback 到内容派生
    if !content_fallback_enabled {
        return SessionUuidResult {
            session_id: None,
            source: "none".to_string(),
            original_value: None,
        };
    }

    // 验证必需参数
    let secret = match secret {
        Some(s) if !s.is_empty() => s,
        _ => {
            return SessionUuidResult {
                session_id: None,
                source: "none".to_string(),
                original_value: None,
            };
        }
    };

    let auth_ns = auth_namespace.unwrap_or("");
    let user = client_user.unwrap_or("");
    let text = match first_user_text {
        Some(t) if !t.is_empty() => t,
        _ => {
            return SessionUuidResult {
                session_id: None,
                source: "none".to_string(),
                original_value: None,
            };
        }
    };

    // 派生 session ID
    let derived_id = match derive_content_session_id(
        secret,
        auth_ns,
        user,
        text,
        256 * 1024, // 256KB max
    ) {
        Some(id) => id,
        None => {
            return SessionUuidResult {
                session_id: None,
                source: "none".to_string(),
                original_value: None,
            };
        }
    };

    // 阶段 3: 派生 session → UUID 映射
    let uuid_store = match uuid_store {
        Some(store) => store,
        None => {
            // 没有存储，生成临时 UUID
            let temp_uuid = Uuid::new_v4();
            return SessionUuidResult {
                session_id: Some(format!("session_{}", temp_uuid)),
                source: "derived_temp".to_string(),
                original_value: Some(derived_id),
            };
        }
    };

    match derive_to_uuid_fallback(&derived_id, uuid_store, ttl_seconds) {
        Ok((mapped_uuid, source)) => SessionUuidResult {
            session_id: Some(format!("session_{}", mapped_uuid)),
            source,
            original_value: Some(derived_id),
        },
        Err(e) => {
            tracing::warn!(error = %e, "Failed to map derived session to UUID; generating temporary UUID");
            let _temp_uuid = Uuid::new_v4();
            SessionUuidResult {
                session_id: Some(format!("session_{}", _temp_uuid)),
                source: "derived_temp".to_string(),
                original_value: Some(derived_id),
            }
        }
    }
}

/// 简化的辅助函数 - 从 HeaderMap 和 payload 解析 session UUID
///
/// 对齐 Python 的 `_resolve_session_uuid` 辅助函数，但使用同步实现。
/// 参数从 AppState / BusinessDefaults 提取。
pub fn resolve_session_uuid_from_request(
    headers: &HeaderMap,
    payload: &serde_json::Value,
    uuid_store: Option<&SessionUuidStore>,
    content_fallback_enabled: bool,
    derivation_secret: Option<&str>,
    auth_namespace: Option<&str>,
    ttl_seconds: i64,
) -> SessionUuidResult {
    // 从 payload 提取 prompt_cache_key
    let prompt_cache_key = payload
        .get("prompt_cache_key")
        .and_then(|v| v.as_str());

    // 提取 first_user_text（用于内容派生）
    let first_user_text = if content_fallback_enabled {
        payload
            .get("messages")
            .and_then(|msgs| msgs.as_array())
            .and_then(|arr| {
                arr.iter().find_map(|msg| {
                    if msg.get("role")?.as_str()? == "user" {
                        // content 可能是 string 或 array
                        match msg.get("content")? {
                            serde_json::Value::String(s) => Some(s.clone()),
                            serde_json::Value::Array(blocks) => {
                                // 从 content blocks 中提取第一个 text
                                blocks.iter().find_map(|block| {
                                    if block.get("type")?.as_str()? == "text" {
                                        block.get("text")?.as_str().map(|s| s.to_string())
                                    } else {
                                        None
                                    }
                                })
                            }
                            _ => None,
                        }
                    } else {
                        None
                    }
                })
            })
    } else {
        None
    };

    resolve_session_uuid(
        headers,
        prompt_cache_key,
        uuid_store,
        content_fallback_enabled,
        derivation_secret,
        auth_namespace,
        None, // client_user - 未使用
        first_user_text.as_deref(),
        ttl_seconds,
    )
}
