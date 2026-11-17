//! 已加载团队配置的只读证据；哈希覆盖应用写入的角色声明和委派提示词。
use codex_app_server_protocol::ThreadAgentTeamConfigEvidence;
use codex_app_server_protocol::ThreadAgentTeamPolicy;
use codex_core::config::Config;
use codex_features::Feature;
use sha2::Digest;
use sha2::Sha256;

/// JSON 值已由字符串/空值构造，序列化不执行文件读取或配置刷新。
fn fingerprint(value: serde_json::Value) -> String {
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

pub(super) fn snapshot(config: &Config) -> ThreadAgentTeamPolicy {
    let role_fingerprints = config
        .agent_roles
        .iter()
        .map(|(name, role)| {
            // 非 UTF-8 路径无法与 JSON 请求建立一致证据，保持未知而非有损转换。
            let path = match role.config_file.as_ref() {
                Some(path) => Some(path.to_str()?),
                None => None,
            };
            Some((
                name.clone(),
                fingerprint(serde_json::json!([role.description, path])),
            ))
        })
        .collect::<Option<std::collections::BTreeMap<_, _>>>();
    if role_fingerprints.is_none() {
        tracing::warn!(
            operation = "thread.read.team_policy",
            error_code = "team_policy.non_utf8_path",
            "team configuration evidence unavailable"
        );
    }
    ThreadAgentTeamPolicy {
        agents_enabled: config.agents_enabled,
        multi_agent_v2_enabled: config.features.enabled(Feature::MultiAgentV2),
        config_evidence: role_fingerprints.map(|role_fingerprints| ThreadAgentTeamConfigEvidence {
            role_fingerprints,
            mode_hint_fingerprint: fingerprint(serde_json::json!(
                config.multi_agent_v2.multi_agent_mode_hint_text
            )),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn snapshot_uses_loaded_configuration_without_reading_role_files() -> anyhow::Result<()> {
        let home = tempfile::tempdir()?;
        let manager = crate::config_manager::ConfigManager::without_managed_config_for_tests(
            home.path().to_path_buf(),
        );
        let mut config = manager
            .load_latest_config(Some(home.path().to_path_buf()))
            .await?;
        config.agents_enabled = false;
        config
            .features
            .enable(Feature::MultiAgentV2)
            .expect("enable multi-agent v2 in synthetic configuration");
        config.agent_roles.clear();
        let role = config.agent_roles.entry("reviewer".into()).or_default();
        role.description = Some("synthetic review role".into());
        role.config_file = Some(home.path().join("not-created.toml"));
        config.multi_agent_v2.multi_agent_mode_hint_text = Some("synthetic hint".into());

        let policy = snapshot(&config);
        assert!(!policy.agents_enabled);
        assert!(policy.multi_agent_v2_enabled);
        let evidence = policy.config_evidence.expect("UTF-8 config evidence");
        assert_eq!(evidence.role_fingerprints.len(), 1);
        assert_eq!(
            evidence.role_fingerprints["reviewer"],
            fingerprint(serde_json::json!([
                "synthetic review role",
                home.path().join("not-created.toml")
            ]))
        );
        assert_eq!(
            evidence.mode_hint_fingerprint,
            fingerprint(serde_json::json!("synthetic hint"))
        );

        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            config.agent_roles.get_mut("reviewer").unwrap().config_file =
                Some(std::ffi::OsString::from_vec(vec![0xff]).into());
            let policy = snapshot(&config);
            assert!(policy.config_evidence.is_none());
            assert!(!policy.agents_enabled);
            assert!(policy.multi_agent_v2_enabled);
        }
        Ok(())
    }

    #[test]
    fn role_and_hint_changes_have_distinct_evidence() {
        let role = fingerprint(serde_json::json!(["review", "/synthetic/reviewer.toml"]));
        assert_ne!(
            role,
            fingerprint(serde_json::json!(["write", "/synthetic/reviewer.toml"]))
        );
        assert_ne!(
            role,
            fingerprint(serde_json::json!(["review", "/synthetic/writer.toml"]))
        );
        assert_ne!(
            fingerprint(serde_json::json!(null)),
            fingerprint(serde_json::json!(""))
        );
        assert_eq!(
            fingerprint(serde_json::json!(null)),
            "74234e98afe7498fb5daf1f36ac2d78acc339464f950703b8c019892f982b90b"
        );
    }
}
