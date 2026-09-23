use crate::{
    Error, Result,
    credentials::Secret,
    transport::{Response, Transport},
};
use bexos_pkg_config::Repository;
use sha2::{Digest, Sha256};
pub const MAX_METADATA: usize = 1024 * 1024;
pub struct Oci<T> {
    pub transport: T,
    pub repository: Repository,
    pub token: Option<Secret>,
    pub transfers: crate::transfers::Transfers,
    pub credential_generation: u64,
}
impl<T: Transport + Clone + 'static> Oci<T> {
    pub async fn role(&mut self, tag: &str, role: &str) -> Result<Vec<u8>> {
        let path = format!("/v2/{}/manifests/{tag}", self.repository.repository);
        let manifest = self.fetch(&path, MAX_METADATA).await?;
        let json: serde_json::Value =
            serde_json::from_slice(&manifest).map_err(|_| Error::VerifyFailed)?;
        if json["schemaVersion"] != 2
            || json["mediaType"] != "application/vnd.oci.image.manifest.v1+json"
            || json["artifactType"] != "application/vnd.bexos.tuf.role.v1"
        {
            return Err(Error::VerifyFailed);
        }
        let config = &json["config"];
        if config["mediaType"] != "application/vnd.oci.empty.v1+json"
            || config["size"] != 2
            || config["digest"] != format!("sha256:{}", hex(&Sha256::digest(b"{}")))
        {
            return Err(Error::VerifyFailed);
        }
        let layers = json["layers"]
            .as_array()
            .filter(|layers| layers.len() == 1)
            .ok_or(Error::VerifyFailed)?;
        let layer = &layers[0];
        if layer["mediaType"] != format!("application/vnd.bexos.tuf.{role}.v1+json") {
            return Err(Error::VerifyFailed);
        }
        let digest = parse_digest(layer["digest"].as_str().ok_or(Error::VerifyFailed)?)?;
        let size = layer["size"]
            .as_u64()
            .filter(|size| *size > 0 && *size <= MAX_METADATA as u64)
            .ok_or(Error::VerifyFailed)?;
        self.blob(&digest, size).await
    }
    pub async fn blob(&mut self, digest: &[u8; 32], size: u64) -> Result<Vec<u8>> {
        if size == 0 || size > 256 * 1024 * 1024 {
            return Err(Error::ResourceExhausted);
        }
        let path = format!(
            "/v2/{}/blobs/sha256:{}",
            self.repository.repository,
            hex(digest)
        );
        let bytes = self.fetch(&path, size as usize).await?;
        if bytes.len() as u64 != size || <[u8; 32]>::from(Sha256::digest(&bytes)) != *digest {
            return Err(Error::VerifyFailed);
        }
        Ok(bytes)
    }
    pub async fn payload(
        &mut self,
        digest: &[u8; 32],
        size: u64,
    ) -> Result<std::rc::Rc<crate::payload::Payload>> {
        let key = format!(
            "{}:{}:{}:{}",
            self.repository.id(),
            hex(digest),
            size,
            self.credential_generation
        );
        let digest = *digest;
        self.transfers
            .get(key, || {
                let mut oci = Self {
                    transport: self.transport.clone(),
                    repository: self.repository.clone(),
                    token: self
                        .token
                        .as_ref()
                        .map(|token| Secret::new(token.bytes().to_vec())),
                    transfers: self.transfers.clone(),
                    credential_generation: self.credential_generation,
                };
                Box::pin(async move {
                    let path = format!(
                        "/v2/{}/blobs/sha256:{}",
                        oci.repository.repository,
                        hex(&digest)
                    );
                    let mut payload = crate::payload::Payload::new(size as usize)?;
                    oci.fetch_into(&path, size as usize, &mut |status, bytes| {
                        if status == 200 {
                            payload.write(bytes)?;
                        }
                        Ok(())
                    })
                    .await?;
                    payload.seal(&digest)
                })
            })
            .await
    }
    async fn fetch(&mut self, path: &str, maximum: usize) -> Result<Vec<u8>> {
        let mut body = Vec::new();
        self.fetch_into(path, maximum, &mut |status, bytes| {
            if status == 200 {
                body.extend_from_slice(bytes);
            }
            Ok(())
        })
        .await?;
        Ok(body)
    }
    async fn fetch_into(
        &mut self,
        path: &str,
        maximum: usize,
        sink: &mut impl FnMut(u16, &[u8]) -> Result<()>,
    ) -> Result<()> {
        let origin = format!("https://{}", self.repository.host);
        let mut current_host = self.repository.host.clone();
        let mut current_path = path.to_string();
        let mut challenged = false;
        for _ in 0..=4 {
            let mut headers = vec![(
                "Accept".into(),
                "application/vnd.oci.image.manifest.v1+json, application/octet-stream".into(),
            )];
            if current_host == self.repository.host {
                if let Some(token) = &self.token {
                    headers.push((
                        "Authorization".into(),
                        format!(
                            "Bearer {}",
                            core::str::from_utf8(token.bytes()).map_err(|_| Error::InvalidArgs)?
                        ),
                    ));
                }
            }
            let response = self
                .transport
                .get_into(&current_host, &current_path, &headers, maximum, sink)
                .await?;
            match response.status {
                200 => {
                    if response.body.len() > maximum {
                        return Err(Error::ResourceExhausted);
                    }
                    return Ok(());
                }
                404 => return Err(Error::NotFound),
                401 if !challenged && current_host == self.repository.host => {
                    self.challenge(&response).await?;
                    challenged = true;
                }
                301 | 302 | 303 | 307 | 308 => {
                    let location = response.header("location").ok_or(Error::VerifyFailed)?;
                    let absolute;
                    let location = if location.starts_with('/') && !location.starts_with("//") {
                        absolute = format!("https://{current_host}{location}");
                        absolute.as_str()
                    } else {
                        location
                    };
                    let (host, path) = https_parts(location)?;
                    let next_origin = format!("https://{host}");
                    if next_origin != origin
                        && !self.repository.redirect_origins.contains(&next_origin)
                    {
                        return Err(Error::AccessDenied);
                    }
                    current_host = host.into();
                    current_path = path.into();
                }
                401 | 403 => return Err(Error::AccessDenied),
                _ => return Err(Error::Unavailable),
            }
        }
        Err(Error::Unavailable)
    }
    async fn challenge(&mut self, response: &Response) -> Result<()> {
        let challenge = response
            .header("www-authenticate")
            .and_then(|header| header.split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
            .map(|(_, fields)| fields.trim_start())
            .ok_or(Error::AccessDenied)?;
        let mut fields = std::collections::BTreeMap::new();
        for field in challenge.split(',') {
            let (key, value) = field.trim().split_once('=').ok_or(Error::VerifyFailed)?;
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .ok_or(Error::VerifyFailed)?;
            if value.contains(['"', '\\', '\r', '\n']) || fields.insert(key, value).is_some() {
                return Err(Error::VerifyFailed);
            }
        }
        let realm = *fields.get("realm").ok_or(Error::AccessDenied)?;
        let (host, path) = https_parts(realm)?;
        if !self
            .repository
            .token_origins
            .contains(&format!("https://{host}"))
            || path.contains('?')
        {
            return Err(Error::AccessDenied);
        }
        let scope = format!("repository:{}:pull", self.repository.repository);
        if fields.get("scope").is_some_and(|value| **value != scope) {
            return Err(Error::AccessDenied);
        }
        let path = format!(
            "{path}?service={}&scope={}",
            encode(
                fields
                    .get("service")
                    .copied()
                    .unwrap_or(&self.repository.host)
            ),
            encode(&scope)
        );
        // Registry bearer credentials are never forwarded to a token issuer.
        let response = self.transport.get(host, &path, &[], 16 * 1024).await?;
        if response.status != 200 {
            return Err(Error::AccessDenied);
        }
        let value: serde_json::Value =
            serde_json::from_slice(&response.body).map_err(|_| Error::VerifyFailed)?;
        let token = value
            .get("token")
            .or_else(|| value.get("access_token"))
            .and_then(|v| v.as_str())
            .ok_or(Error::AccessDenied)?;
        if token.is_empty() || token.len() > 8192 || !token.bytes().all(|b| (33..=126).contains(&b))
        {
            return Err(Error::VerifyFailed);
        }
        self.token = Some(Secret::new(token.as_bytes().to_vec()));
        Ok(())
    }
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
pub fn parse_digest(value: &str) -> Result<[u8; 32]> {
    let value = value.strip_prefix("sha256:").ok_or(Error::VerifyFailed)?;
    if value.len() != 64
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::VerifyFailed);
    }
    let mut digest = [0; 32];
    for (i, byte) in digest.iter_mut().enumerate() {
        *byte =
            u8::from_str_radix(&value[i * 2..i * 2 + 2], 16).map_err(|_| Error::VerifyFailed)?;
    }
    Ok(digest)
}
fn https_parts(url: &str) -> Result<(&str, &str)> {
    let rest = url.strip_prefix("https://").ok_or(Error::AccessDenied)?;
    let split = rest.find('/').ok_or(Error::InvalidArgs)?;
    let (host, path) = rest.split_at(split);
    if !bexos_pkg_client::valid_host(host)
        || path.contains('#')
        || path.bytes().any(|b| b <= 32 || b == 127)
    {
        return Err(Error::InvalidArgs);
    }
    Ok((host, path))
}
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
