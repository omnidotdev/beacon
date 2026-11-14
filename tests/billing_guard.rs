//! Guard: Beacon never prices, marks up, meters, or deducts managed AI spend
//!
//! The Synapse gateway is the only place cost becomes credits (with the
//! product margin) and the only place credits are checked and deducted. Beacon
//! keeps no ledger, no markup literal, and no usage recorder of its own, and
//! its local price table only feeds the operator's usage log

use std::fs;
use std::path::{Path, PathBuf};

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn sources() -> Vec<(String, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_sources(&root.join("src"), &mut files);
    files
        .into_iter()
        .map(|path| {
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            (rel, fs::read_to_string(&path).unwrap())
        })
        .collect()
}

#[test]
fn no_markup_or_cogs_literal() {
    for (path, text) in sources() {
        // Telegram's "reply markup" is message formatting, not pricing
        let lower = text
            .to_lowercase()
            .replace("reply_markup", "")
            .replace("keyboardmarkup", "")
            .replace("keyboard markup", "");
        for needle in ["markup", "cogs"] {
            assert!(
                !lower.contains(needle),
                "{path} mentions `{needle}`: credit markup lives only in the Synapse gateway"
            );
        }
    }
}

#[test]
fn no_local_metering_or_credit_ledger() {
    let forbidden = [
        "UsageRecorder",
        "record_usage(",
        "deduct_credits(",
        "add_credits(",
        "check_credits(",
        "credits_from_cost",
        "credits_remaining",
    ];
    for (path, text) in sources() {
        for needle in forbidden {
            assert!(
                !text.contains(needle),
                "{path} contains `{needle}`: the Synapse gateway meters and charges managed AI"
            );
        }
    }
}

#[test]
fn local_price_table_only_feeds_the_usage_log() {
    let users: Vec<String> = sources()
        .into_iter()
        .filter(|(path, text)| path != "src/billing/pricing.rs" && text.contains("pricing::"))
        .map(|(path, _)| path)
        .collect();
    assert_eq!(
        users,
        vec!["src/agent/runner.rs".to_string()],
        "the local price estimate must only reach the local usage log"
    );
}

#[test]
fn app_id_is_never_configurable_or_synapse() {
    for (path, text) in sources() {
        for needle in ["var(\"AETHER_APP_ID\")", "var(\"BILLING_APP_ID\")"] {
            assert!(
                !text.contains(needle),
                "{path} reads `{needle}`: Beacon always bills as `beacon`"
            );
        }
    }
    let billing =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/billing/mod.rs"))
            .unwrap();
    assert!(billing.contains("pub const BEACON_APP_ID: &str = \"beacon\";"));
}

/// Files on the memory AI path (embeddings, fact extraction, query condensing)
const MEMORY_AI_PATH: &[&str] = &[
    "src/api/mod.rs",
    "src/api/managed.rs",
    "src/api/websocket.rs",
    "src/api/webhooks/vortex.rs",
    "src/api/webhooks/telegram/process.rs",
    "src/agent/runner.rs",
    "src/context/compaction.rs",
    "src/db/indexer.rs",
    "src/db/memory_ai.rs",
    "src/tools/memory.rs",
];

#[test]
fn memory_ai_never_calls_a_provider_directly() {
    // Memory AI runs through the Synapse gateway on the user's product-scoped
    // key (cloud) or the operator's gateway (self-host), never on a raw
    // provider key held by the server
    let sources = sources();
    for file in MEMORY_AI_PATH {
        let (_, text) = sources
            .iter()
            .find(|(path, _)| path == file)
            .unwrap_or_else(|| panic!("{file} is missing: update MEMORY_AI_PATH"));
        for needle in [
            "OPENAI_API_KEY",
            "api.openai.com",
            "Embedder::new",
            "Embedder::with_model",
            "LlmCondenser",
        ] {
            assert!(
                !text.contains(needle),
                "{file} contains `{needle}`: memory AI must go through the Synapse gateway"
            );
        }
    }
}

#[test]
fn no_server_side_openai_embedder_anywhere() {
    for (path, text) in sources() {
        assert!(
            !text.contains("Embedder::new") && !text.contains("LlmCondenser::new"),
            "{path} builds a direct provider embedder or condenser"
        );
    }
}

#[test]
fn no_direct_provider_api_calls() {
    // Managed AI (chat, memory, vision, transcription) runs through a Synapse
    // gateway that meters it; only the embedded gateway's own provider config
    // may name a provider endpoint
    let hosts = [
        "api.openai.com",
        "api.anthropic.com",
        "generativelanguage.googleapis.com",
        "api.deepgram.com",
    ];
    for (path, text) in sources() {
        if path == "src/config/synapse_bridge.rs" {
            continue;
        }
        for host in hosts {
            assert!(
                !text.contains(host),
                "{path} calls `{host}` directly: route it through the Synapse gateway"
            );
        }
    }
}
