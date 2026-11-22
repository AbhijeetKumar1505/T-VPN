//! A small AWS client for `tvpn cloud`: EC2 (instances, security group),
//! SSM (latest Ubuntu image) and STS (credential check). Blocking HTTP; call
//! it from `spawn_blocking` in async code.

use super::{sigv4, xml};
use crate::paths;
use anyhow::{Context, Result, anyhow, bail};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

const EC2_VERSION: &str = "2016-11-15";
pub const SECURITY_GROUP: &str = "tvpn-wireguard";
pub const TAG_KEY: &str = "tvpn";
pub const TAG_VALUE: &str = "cloud";
const UBUNTU_SSM_PARAMETER: &str =
    "/aws/service/canonical/ubuntu/server/24.04/stable/current/amd64/hvm/ebs-gp3/ami-id";
const CANONICAL_OWNER: &str = "099720109477";
const UBUNTU_NAME_PATTERN: &str = "ubuntu/images/hvm-ssd-gp3/ubuntu-noble-24.04-amd64-server-*";

/// Minimal IAM policy for `tvpn cloud` (shown by `tvpn cloud setup`).
pub const IAM_POLICY: &str = r#"{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": [
        "ec2:RunInstances",
        "ec2:CreateTags",
        "ec2:DescribeInstances",
        "ec2:DescribeImages",
        "ec2:DescribeSecurityGroups",
        "ec2:CreateSecurityGroup",
        "ec2:AuthorizeSecurityGroupIngress",
        "ssm:GetParameter"
      ],
      "Resource": "*"
    },
    {
      "Effect": "Allow",
      "Action": "ec2:TerminateInstances",
      "Resource": "*",
      "Condition": { "StringEquals": { "ec2:ResourceTag/tvpn": "cloud" } }
    }
  ]
}"#;

#[derive(Clone, Serialize, Deserialize)]
pub struct Credentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
    /// Where these came from (for messages); not stored.
    #[serde(skip)]
    pub source: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Credentials({}, from {})", self.access_key_id, self.source)
    }
}

fn tvpn_credentials_file() -> PathBuf {
    paths::config_dir().join("cloud").join("aws.json")
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    if let Some(home) = paths::sudo_user_home() {
        return Some(home);
    }
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}

fn shared_credentials_file() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("AWS_SHARED_CREDENTIALS_FILE") {
        return Some(PathBuf::from(p));
    }
    home_dir().map(|h| h.join(".aws").join("credentials"))
}

/// Parse a profile from an AWS shared credentials (INI) file.
fn parse_shared_credentials(text: &str, profile: &str) -> Option<Credentials> {
    let mut in_profile = false;
    let (mut id, mut secret, mut token) = (None, None, None);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_profile = name.trim() == profile;
            continue;
        }
        if !in_profile {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim().to_string();
            match k.trim().to_ascii_lowercase().as_str() {
                "aws_access_key_id" => id = Some(v),
                "aws_secret_access_key" => secret = Some(v),
                "aws_session_token" => token = Some(v),
                _ => {}
            }
        }
    }
    Some(Credentials {
        access_key_id: id?,
        secret_access_key: secret?,
        session_token: token.filter(|t| !t.is_empty()),
        source: String::new(),
    })
}

impl Credentials {
    /// Environment variables, then `tvpn cloud setup`'s file, then `~/.aws/credentials`.
    pub fn load() -> Result<Credentials> {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        if let (Some(id), Some(secret)) = (env("AWS_ACCESS_KEY_ID"), env("AWS_SECRET_ACCESS_KEY")) {
            return Ok(Credentials {
                access_key_id: id,
                secret_access_key: secret,
                session_token: env("AWS_SESSION_TOKEN"),
                source: "environment variables".into(),
            });
        }
        let file = tvpn_credentials_file();
        if let Ok(text) = std::fs::read_to_string(&file) {
            let mut c: Credentials =
                serde_json::from_str(&text).with_context(|| format!("invalid {}", file.display()))?;
            c.source = file.display().to_string();
            return Ok(c);
        }
        if let Some(path) = shared_credentials_file()
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            let profile = env("AWS_PROFILE").unwrap_or_else(|| "default".into());
            if let Some(mut c) = parse_shared_credentials(&text, &profile) {
                c.source = format!("{} [{profile}]", path.display());
                return Ok(c);
            }
        }
        let sudo_note = if cfg!(unix) && std::env::var_os("SUDO_USER").is_some() {
            "\n  (AWS_* environment variables are not passed through sudo; use `tvpn cloud setup` instead)"
        } else {
            ""
        };
        bail!("no AWS credentials found. Run `tvpn cloud setup` once to add them.{sudo_note}")
    }

    /// Store credentials for tvpn (owner-only file). Returns the path.
    pub fn save(&self) -> Result<PathBuf> {
        let path = tvpn_credentials_file();
        let stored = Credentials { source: String::new(), ..self.clone() };
        paths::write_private(&path, serde_json::to_string_pretty(&stored)?.as_bytes())?;
        Ok(path)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub id: String,
    pub state: String,
    pub public_ip: Option<IpAddr>,
    pub launch_time: Option<String>,
    pub region: String,
}

#[derive(Clone)]
pub struct Aws {
    creds: Credentials,
    region: String,
    agent: ureq::Agent,
}

impl Aws {
    pub fn new(creds: Credentials, region: &str) -> Aws {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .http_status_as_error(false)
            .user_agent(concat!("tvpn/", env!("CARGO_PKG_VERSION")))
            .build();
        Aws { creds, region: region.to_string(), agent: ureq::Agent::new_with_config(config) }
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    /// Signed POST; returns (HTTP status, body).
    fn post(
        &self,
        host: &str,
        service: &str,
        region: &str,
        content_type: &str,
        extra: &[(&str, String)],
        body: &[u8],
    ) -> Result<(u16, String)> {
        let (_, amz_date) = sigv4::amz_dates(SystemTime::now());
        let mut headers: Vec<(&str, String)> = vec![
            ("content-type", content_type.into()),
            ("host", host.into()),
            ("x-amz-date", amz_date.clone()),
        ];
        headers.extend(extra.iter().cloned());
        if let Some(token) = &self.creds.session_token {
            headers.push(("x-amz-security-token", token.clone()));
        }
        let sig = sigv4::sign(
            &self.creds.access_key_id,
            &self.creds.secret_access_key,
            &sigv4::Request {
                method: "POST",
                path: "/",
                query: "",
                headers: &headers,
                payload: body,
                region,
                service,
                amz_date: &amz_date,
            },
        );
        let mut req = self.agent.post(format!("https://{host}/"));
        for (k, v) in &headers {
            if *k != "host" {
                req = req.header(*k, v.as_str());
            }
        }
        req = req.header("authorization", sig.authorization.as_str());
        let mut resp = req.send(body).map_err(|e| anyhow!("cannot reach AWS ({host}): {e}"))?;
        let status = resp.status().as_u16();
        let text = resp
            .body_mut()
            .with_config()
            .limit(16 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| anyhow!("reading the AWS response failed: {e}"))?;
        Ok((status, text))
    }

    /// Call an AWS Query API action (EC2, STS) and return the XML response.
    fn query(
        &self,
        host: &str,
        service: &str,
        region: &str,
        version: &str,
        action: &str,
        params: &[(String, String)],
    ) -> Result<String> {
        let mut all =
            vec![("Action".to_string(), action.to_string()), ("Version".to_string(), version.to_string())];
        all.extend(params.iter().cloned());
        let body = sigv4::form_body(&all);
        let (status, text) = self.post(
            host,
            service,
            region,
            "application/x-www-form-urlencoded; charset=utf-8",
            &[],
            body.as_bytes(),
        )?;
        if status >= 400 {
            let code = xml::first(&text, "Code").unwrap_or_else(|| format!("HTTP {status}"));
            let message = xml::first(&text, "Message").unwrap_or_default();
            return Err(aws_error(&code, &message, &self.region));
        }
        Ok(text)
    }

    fn ec2(&self, action: &str, params: &[(String, String)]) -> Result<String> {
        let host = format!("ec2.{}.amazonaws.com", self.region);
        self.query(&host, "ec2", &self.region, EC2_VERSION, action, params)
    }

    /// AWS account ID of these credentials (works without any IAM permission).
    pub fn account_id(&self) -> Result<String> {
        let xml =
            self.query("sts.amazonaws.com", "sts", "us-east-1", "2011-06-15", "GetCallerIdentity", &[])?;
        xml::first(&xml, "Account").ok_or_else(|| anyhow!("unexpected STS response"))
    }

    /// Latest Ubuntu 24.04 LTS (amd64) image in this region.
    pub fn ubuntu_image(&self) -> Result<String> {
        match self.ubuntu_image_from_ssm() {
            Ok(id) => Ok(id),
            Err(ssm_err) => {
                tracing::debug!("SSM image lookup failed ({ssm_err:#}); falling back to DescribeImages");
                self.ubuntu_image_from_ec2()
            }
        }
    }

    fn ubuntu_image_from_ssm(&self) -> Result<String> {
        let host = format!("ssm.{}.amazonaws.com", self.region);
        let body = serde_json::json!({ "Name": UBUNTU_SSM_PARAMETER }).to_string();
        let (status, text) = self.post(
            &host,
            "ssm",
            &self.region,
            "application/x-amz-json-1.1",
            &[("x-amz-target", "AmazonSSM.GetParameter".to_string())],
            body.as_bytes(),
        )?;
        let json: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
        if status >= 400 {
            let code =
                json["__type"].as_str().unwrap_or("error").rsplit('#').next().unwrap_or("error").to_string();
            let message =
                json["message"].as_str().or(json["Message"].as_str()).unwrap_or_default().to_string();
            return Err(aws_error(&code, &message, &self.region));
        }
        json["Parameter"]["Value"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| anyhow!("unexpected SSM response"))
    }

    fn ubuntu_image_from_ec2(&self) -> Result<String> {
        let params = vec![
            ("Owner.1".into(), CANONICAL_OWNER.into()),
            ("Filter.1.Name".into(), "name".into()),
            ("Filter.1.Value.1".into(), UBUNTU_NAME_PATTERN.into()),
            ("Filter.2.Name".into(), "state".into()),
            ("Filter.2.Value.1".into(), "available".into()),
        ];
        let body = self.ec2("DescribeImages", &params)?;
        xml::chunks_starting_at(&body, "imageId")
            .into_iter()
            .filter_map(|c| {
                Some((xml::first(c, "creationDate").unwrap_or_default(), xml::first(c, "imageId")?))
            })
            .max()
            .map(|(_, id)| id)
            .ok_or_else(|| anyhow!("no Ubuntu 24.04 image found in {}", self.region))
    }

    /// Find or create the security group that lets WireGuard (UDP) in.
    pub fn ensure_security_group(&self, port: u16) -> Result<String> {
        let found = self.ec2(
            "DescribeSecurityGroups",
            &[
                ("Filter.1.Name".into(), "group-name".into()),
                ("Filter.1.Value.1".into(), SECURITY_GROUP.into()),
            ],
        )?;
        let group_id = match xml::first(&found, "groupId") {
            Some(id) => id,
            None => {
                let created = self.ec2(
                    "CreateSecurityGroup",
                    &[
                        ("GroupName".into(), SECURITY_GROUP.into()),
                        ("GroupDescription".into(), "tvpn on-demand WireGuard server (UDP only)".into()),
                    ],
                )?;
                xml::first(&created, "groupId")
                    .ok_or_else(|| anyhow!("unexpected CreateSecurityGroup response"))?
            }
        };
        let port = port.to_string();
        let rule = self.ec2(
            "AuthorizeSecurityGroupIngress",
            &[
                ("GroupId".into(), group_id.clone()),
                ("IpPermissions.1.IpProtocol".into(), "udp".into()),
                ("IpPermissions.1.FromPort".into(), port.clone()),
                ("IpPermissions.1.ToPort".into(), port),
                ("IpPermissions.1.IpRanges.1.CidrIp".into(), "0.0.0.0/0".into()),
                ("IpPermissions.1.Ipv6Ranges.1.CidrIpv6".into(), "::/0".into()),
            ],
        );
        match rule {
            Ok(_) => {}
            Err(e) if format!("{e:#}").contains("InvalidPermission.Duplicate") => {}
            Err(e) => return Err(e),
        }
        Ok(group_id)
    }

    /// Launch one tagged instance; returns its ID.
    pub fn run_instance(
        &self,
        image: &str,
        instance_type: &str,
        security_group: &str,
        user_data: &str,
        name: &str,
    ) -> Result<String> {
        let mut params: Vec<(String, String)> = vec![
            ("ImageId".into(), image.into()),
            ("InstanceType".into(), instance_type.into()),
            ("MinCount".into(), "1".into()),
            ("MaxCount".into(), "1".into()),
            ("SecurityGroupId.1".into(), security_group.into()),
            ("UserData".into(), base64::engine::general_purpose::STANDARD.encode(user_data)),
            ("InstanceInitiatedShutdownBehavior".into(), "terminate".into()),
            ("MetadataOptions.HttpTokens".into(), "required".into()),
            ("MetadataOptions.HttpEndpoint".into(), "enabled".into()),
        ];
        if instance_type.starts_with('t') {
            // Burstable types default to "unlimited" CPU credits, which can bill extra.
            params.push(("CreditSpecification.CpuCredits".into(), "standard".into()));
        }
        for (n, resource) in ["instance", "volume"].iter().enumerate() {
            let p = format!("TagSpecification.{}", n + 1);
            params.push((format!("{p}.ResourceType"), resource.to_string()));
            params.push((format!("{p}.Tag.1.Key"), "Name".into()));
            params.push((format!("{p}.Tag.1.Value"), name.into()));
            params.push((format!("{p}.Tag.2.Key"), TAG_KEY.into()));
            params.push((format!("{p}.Tag.2.Value"), TAG_VALUE.into()));
        }
        let body = self.ec2("RunInstances", &params)?;
        xml::first(&body, "instanceId").ok_or_else(|| anyhow!("unexpected RunInstances response"))
    }

    fn parse_instances(&self, body: &str) -> Vec<Instance> {
        xml::chunks_starting_at(body, "instanceId")
            .into_iter()
            .filter_map(|c| {
                Some(Instance {
                    id: xml::first(c, "instanceId")?,
                    state: xml::first_raw(c, "instanceState")
                        .and_then(|s| xml::first(s, "name"))
                        .unwrap_or_default(),
                    public_ip: xml::first(c, "ipAddress").and_then(|ip| ip.parse().ok()),
                    launch_time: xml::first(c, "launchTime"),
                    region: self.region.clone(),
                })
            })
            .collect()
    }

    pub fn instance(&self, id: &str) -> Result<Instance> {
        let body = self.ec2("DescribeInstances", &[("InstanceId.1".into(), id.into())])?;
        self.parse_instances(&body).into_iter().next().ok_or_else(|| anyhow!("instance {id} not found"))
    }

    /// tvpn-created instances in this region that are not terminated.
    pub fn tvpn_instances(&self) -> Result<Vec<Instance>> {
        let mut params: Vec<(String, String)> = vec![
            ("Filter.1.Name".into(), format!("tag:{TAG_KEY}")),
            ("Filter.1.Value.1".into(), TAG_VALUE.into()),
            ("Filter.2.Name".into(), "instance-state-name".into()),
        ];
        for (i, state) in ["pending", "running", "stopping", "stopped"].iter().enumerate() {
            params.push((format!("Filter.2.Value.{}", i + 1), state.to_string()));
        }
        let body = self.ec2("DescribeInstances", &params)?;
        Ok(self.parse_instances(&body))
    }

    pub fn terminate(&self, id: &str) -> Result<()> {
        match self.ec2("TerminateInstances", &[("InstanceId.1".into(), id.into())]) {
            Ok(_) => Ok(()),
            Err(e) if format!("{e:#}").contains("InvalidInstanceID.NotFound") => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Turn an AWS error code into a message that says what to do.
fn aws_error(code: &str, message: &str, region: &str) -> anyhow::Error {
    let hint = match code {
        "AuthFailure" | "InvalidClientTokenId" | "SignatureDoesNotMatch" | "UnrecognizedClientException" => {
            "check your AWS access keys (run `tvpn cloud setup` again)".to_string()
        }
        "UnauthorizedOperation" | "AccessDenied" | "AccessDeniedException" => {
            "your AWS user is missing a permission tvpn needs; `tvpn cloud setup` shows the policy to attach".to_string()
        }
        "OptInRequired" => format!(
            "region {region} is not enabled for your account: enable it in the AWS console (account menu > AWS Regions), or pick another region"
        ),
        "PendingVerification" => "your new AWS account is still being verified; try again in a few minutes".into(),
        "InsufficientInstanceCapacity" => {
            "AWS has no capacity for this instance type here right now; try another region or --instance-type t3a.micro".into()
        }
        "VPCIdNotSpecified" | "VPCResourceNotSpecified" | "MissingInput" => {
            "this region has no default VPC; create one in the VPC console (Actions > Create default VPC)".into()
        }
        "Unsupported" | "InvalidParameterCombination" => {
            "this instance type isn't offered here; try --instance-type t3a.micro or t2.micro".into()
        }
        "RequestExpired" => "your computer's clock is off; sync it and try again".into(),
        "Blocked" => "your AWS account is blocked; contact AWS support".into(),
        _ => String::new(),
    };
    let message = if message.is_empty() { String::new() } else { format!(": {message}") };
    if hint.is_empty() {
        anyhow!("AWS error {code}{message}")
    } else {
        anyhow!("AWS error {code}{message}\n  {hint}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_credentials_profiles() {
        let ini = "[default]\naws_access_key_id = AKIADEFAULT\naws_secret_access_key = s1\n\n\
                   [work]\naws_access_key_id=AKIAWORK\naws_secret_access_key=s2\naws_session_token = tok\n";
        let d = parse_shared_credentials(ini, "default").unwrap();
        assert_eq!(
            (d.access_key_id.as_str(), d.secret_access_key.as_str(), d.session_token),
            ("AKIADEFAULT", "s1", None)
        );
        let w = parse_shared_credentials(ini, "work").unwrap();
        assert_eq!(w.session_token.as_deref(), Some("tok"));
        assert!(parse_shared_credentials(ini, "missing").is_none());
    }

    #[test]
    fn debug_hides_the_secret() {
        let c = Credentials {
            access_key_id: "AKIA1".into(),
            secret_access_key: "super-secret".into(),
            session_token: None,
            source: "test".into(),
        };
        assert!(!format!("{c:?}").contains("super-secret"));
    }

    #[test]
    fn parses_instances() {
        let aws = Aws::new(
            Credentials {
                access_key_id: "a".into(),
                secret_access_key: "b".into(),
                session_token: None,
                source: String::new(),
            },
            "ap-south-1",
        );
        let body = "<DescribeInstancesResponse><reservationSet><item><instancesSet><item>\
            <instanceId>i-1</instanceId><instanceState><code>16</code><name>running</name></instanceState>\
            <ipAddress>13.0.0.1</ipAddress><launchTime>2026-01-01T00:00:00.000Z</launchTime>\
            </item></instancesSet></item></reservationSet></DescribeInstancesResponse>";
        let list = aws.parse_instances(body);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].state, "running");
        assert_eq!(list[0].public_ip, Some("13.0.0.1".parse().unwrap()));
        assert_eq!(list[0].region, "ap-south-1");
    }

    #[test]
    fn errors_carry_hints() {
        let e = aws_error("OptInRequired", "You are not subscribed", "ap-south-2").to_string();
        assert!(e.contains("ap-south-2") && e.contains("enable it"), "{e}");
        assert_eq!(aws_error("Weird", "", "x").to_string(), "AWS error Weird");
    }

    #[test]
    fn policy_is_valid_json() {
        let v: serde_json::Value = serde_json::from_str(IAM_POLICY).unwrap();
        assert_eq!(v["Version"], "2012-10-17");
    }
}
