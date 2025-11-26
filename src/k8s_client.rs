//! Kubernetes API client for pod operations.

use anyhow::{anyhow, bail, Context, Result};
use k8s_openapi::api::core::v1::Pod;
use kube::{
    api::{Api, AttachedProcess, ListParams},
    config::KubeConfigOptions,
    Client, Config,
};
use std::time::Duration;
use tokio::io::AsyncReadExt;

pub struct K8sClient {
    client: Client,
}

impl K8sClient {
    pub async fn new(kubeconfig_path: Option<String>) -> Result<Self> {
        let config = if let Some(path) = kubeconfig_path {
            let kube_config = kube::config::Kubeconfig::read_from(&path)
                .context("Failed to read kubeconfig from specified path")?;
            Config::from_custom_kubeconfig(kube_config, &KubeConfigOptions::default())
                .await
                .context("Failed to load kubeconfig from specified path")?
        } else {
            Config::infer()
                .await
                .context("Failed to infer kubernetes configuration")?
        };

        let client = Client::try_from(config)
            .context("Failed to create kubernetes client")?;

        Ok(Self { client })
    }

    /// Find a healthy pod for a deployment in a namespace
    pub async fn find_healthy_pod(
        &self,
        namespace: &str,
        deployment_name: &str,
    ) -> Result<String> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), namespace);

        // List pods with label selector
        let label_selector = format!("app={deployment_name}");
        let lp = ListParams::default().labels(&label_selector);

        let pod_list = pods
            .list(&lp)
            .await
            .context("Failed to list pods")?;

        tracing::debug!(
            "Found {} pods for deployment {} in namespace {}",
            pod_list.items.len(),
            deployment_name,
            namespace
        );

        // Find first healthy pod
        for pod in pod_list.items {
            let pod_name = pod
                .metadata
                .name
                .as_ref()
                .context("Pod has no name")?;

            // Check pod phase
            if let Some(status) = &pod.status {
                if let Some(phase) = &status.phase
                    && phase != "Running"
                {
                    tracing::debug!("Pod {} is not running (phase: {})", pod_name, phase);
                    continue;
                }

                // Check container statuses
                if let Some(container_statuses) = &status.container_statuses {
                    let all_ready = container_statuses
                        .iter()
                        .all(|cs| cs.ready);

                    if all_ready {
                        tracing::info!("Found healthy pod: {}", pod_name);
                        return Ok(pod_name.clone());
                    }
                    tracing::debug!("Pod {} has containers that are not ready", pod_name);
                }
            }
        }

        bail!(
            "No healthy pods found for deployment '{deployment_name}' in namespace '{namespace}'"
        )
    }

    /// Execute a command in a pod with timeout
    pub async fn exec_command_in_pod(
        &self,
        namespace: &str,
        pod_name: &str,
        container_name: &str,
        command: Vec<String>,
        timeout_secs: u64,
    ) -> Result<String> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), namespace);

        tracing::debug!(
            "Executing command in pod {}/{} container {}: {:?}",
            namespace,
            pod_name,
            container_name,
            command
        );

        let attached = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            pods.exec(
                pod_name,
                command,
                &kube::api::AttachParams::default()
                    .container(container_name)
                    .stdout(true)
                    .stderr(true),
            ),
        )
        .await
        .context("Command execution timed out")?
        .context("Failed to execute command in pod")?;

        let output = self.get_output(attached).await?;

        tracing::debug!("Command output: {}", output);
        Ok(output)
    }

    /// Read file content from a pod
    pub async fn read_file_from_pod(
        &self,
        namespace: &str,
        pod_name: &str,
        container_name: &str,
        file_path: &str,
    ) -> Result<String> {
        let command = vec!["cat".to_string(), file_path.to_string()];

        self.exec_command_in_pod(namespace, pod_name, container_name, command, 30)
            .await
            .with_context(|| format!("Failed to read file '{file_path}' from pod"))
    }

    /// Get multiple environment variable values from pod spec in a single API call.
    /// Returns values in the same order as the requested env_var_names.
    pub async fn get_pod_env_vars(
        &self,
        namespace: &str,
        pod_name: &str,
        env_var_names: &[&str],
    ) -> Result<Vec<Option<String>>> {
        let pods: Api<Pod> = Api::namespaced(self.client.clone(), namespace);

        let pod = pods
            .get(pod_name)
            .await
            .context("Failed to get pod")?;

        let mut results: Vec<Option<String>> = vec![None; env_var_names.len()];

        if let Some(spec) = &pod.spec {
            for container in &spec.containers {
                if let Some(env_vars) = &container.env {
                    for env_var in env_vars {
                        for (i, name) in env_var_names.iter().enumerate() {
                            if env_var.name == *name && results[i].is_none() {
                                results[i] = env_var.value.clone();
                            }
                        }
                    }
                }
            }
        }

        Ok(results)
    }

    /// Helper to get output from attached process (captures both stdout and stderr)
    async fn get_output(&self, mut attached: AttachedProcess) -> Result<String> {
        let stdout = attached.stdout().ok_or_else(|| {
            anyhow!("Failed to get stdout from attached process")
        })?;
        let stderr = attached.stderr().ok_or_else(|| {
            anyhow!("Failed to get stderr from attached process")
        })?;

        // Read both stdout and stderr concurrently
        let (stdout_result, stderr_result) = tokio::join!(
            async {
                let mut output = Vec::new();
                let mut reader = tokio::io::BufReader::new(stdout);
                reader.read_to_end(&mut output).await?;
                Ok::<_, std::io::Error>(output)
            },
            async {
                let mut output = Vec::new();
                let mut reader = tokio::io::BufReader::new(stderr);
                reader.read_to_end(&mut output).await?;
                Ok::<_, std::io::Error>(output)
            }
        );

        let stdout_bytes = stdout_result.context("Failed to read stdout")?;
        let stderr_bytes = stderr_result.context("Failed to read stderr")?;

        let stdout_str = String::from_utf8(stdout_bytes)
            .context("stdout is not valid UTF-8")?;
        let stderr_str = String::from_utf8(stderr_bytes)
            .context("stderr is not valid UTF-8")?;

        // If stdout is empty but stderr has content, return stderr (it's an error)
        // If both have content, prefer stdout but append stderr if it looks like an error
        if stdout_str.trim().is_empty() && !stderr_str.trim().is_empty() {
            Ok(stderr_str)
        } else if !stderr_str.trim().is_empty() && stderr_str.contains("Error") {
            // Append error info to stdout
            Ok(format!("{}\n{}", stdout_str, stderr_str))
        } else {
            Ok(stdout_str)
        }
    }
}
