use super::NetworkMountError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct NetworkUriParts {
    pub(super) scheme: String,
    pub(super) host: String,
    pub(super) path: String,
    pub(super) path_segments: Vec<String>,
    pub(super) has_password: bool,
    userinfo: Option<String>,
}

impl NetworkUriParts {
    pub(super) fn canonical_scheme(&self) -> &str {
        match self.scheme.as_str() {
            "http" => "dav",
            "https" => "davs",
            scheme => scheme,
        }
    }

    pub(super) fn normalized(&self) -> String {
        self.uri_with_authority(self.canonical_scheme(), &self.host)
    }

    pub(super) fn to_uri(&self, scheme: &str) -> String {
        self.to_uri_with_encoded_userinfo(scheme, self.userinfo.as_deref())
    }

    pub(super) fn to_uri_with_username(&self, scheme: &str, username: Option<&str>) -> String {
        self.to_uri_with_encoded_userinfo(
            scheme,
            username
                .map(str::trim)
                .filter(|username| !username.is_empty())
                .map(percent_encode_userinfo)
                .as_deref(),
        )
    }

    pub(super) fn uri_without_username(&self, scheme: &str) -> String {
        self.uri_with_authority(scheme, &self.host)
    }

    pub(super) fn username(&self) -> Option<String> {
        self.userinfo
            .as_deref()
            .map(|userinfo| {
                userinfo
                    .split_once(':')
                    .map_or(userinfo, |(username, _)| username)
            })
            .filter(|username| !username.is_empty())
            .map(percent_decode_userinfo)
    }

    fn to_uri_with_encoded_userinfo(&self, scheme: &str, userinfo: Option<&str>) -> String {
        let authority = match userinfo {
            Some(userinfo) => format!("{userinfo}@{}", self.host),
            None => self.host.clone(),
        };
        self.uri_with_authority(scheme, &authority)
    }

    fn uri_with_authority(&self, scheme: &str, authority: &str) -> String {
        let path = self.path.trim_end_matches('/');
        if path.is_empty() {
            format!("{scheme}://{authority}")
        } else {
            format!("{scheme}://{authority}{path}")
        }
    }
}

pub(super) fn parse_network_uri(uri: &str) -> Result<NetworkUriParts, NetworkMountError> {
    let uri = uri.trim();
    let (scheme, rest) = uri
        .split_once("://")
        .ok_or_else(|| NetworkMountError::InvalidUri {
            uri: uri.to_owned(),
            message: "missing scheme".to_owned(),
        })?;
    let scheme = scheme.to_ascii_lowercase();
    let (authority, path) = rest
        .split_once('/')
        .map_or((rest, ""), |(authority, path)| (authority, path));
    let (userinfo, host) = authority
        .rsplit_once('@')
        .map_or((None, authority), |(userinfo, host)| (Some(userinfo), host));
    let has_password = userinfo.is_some_and(|userinfo| userinfo.contains(':'));
    let path = format!("/{path}");
    let path_segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    Ok(NetworkUriParts {
        scheme,
        host: host.to_ascii_lowercase(),
        path,
        path_segments,
        has_password,
        userinfo: userinfo.map(ToOwned::to_owned),
    })
}

pub(super) fn percent_encode_gvfs_prefix(path: &str) -> String {
    let path = if path.is_empty() { "/" } else { path };
    let mut output = String::with_capacity(path.len());
    for byte in path.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                output.push(*byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn percent_encode_userinfo(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                output.push(*byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn percent_decode_userinfo(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &value[index + 1..index + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                output.push(byte);
                index += 3;
                continue;
            }
        }
        output.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}
