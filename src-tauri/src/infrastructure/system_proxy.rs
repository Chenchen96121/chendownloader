use url::Url;

#[derive(Clone, Default)]
pub(crate) struct SystemProxies {
    pub(crate) http: Option<Url>,
    pub(crate) https: Option<Url>,
    pub(crate) bypass: Option<String>,
}

impl SystemProxies {
    pub(crate) fn snapshot(&self) -> crate::application::network_profile::NetworkProfile {
        crate::application::network_profile::NetworkProfile {
            proxies: self.clone(),
            x_video_http: Default::default(),
        }
    }
    /// Native CDN probing has an explicit HTTPS route; never inherit ambient env.
    pub(crate) fn https_for_host(&self, host: &str) -> Option<&Url> {
        if self.bypass.as_deref().is_some_and(|list| {
            list.split(',').any(|entry| {
                entry == "*" || host.eq_ignore_ascii_case(entry) || {
                    let domain = entry.strip_prefix('.').unwrap_or(entry);
                    host.eq_ignore_ascii_case(domain)
                        || host
                            .to_ascii_lowercase()
                            .ends_with(&format!(".{}", domain.to_ascii_lowercase()))
                }
            })
        }) {
            None
        } else {
            self.https.as_ref()
        }
    }
    pub(crate) fn from_windows_manual_settings(enabled: bool, server: &str) -> Self {
        if !enabled || server.len() > 4096 || server.bytes().any(|byte| byte.is_ascii_control()) {
            return Self::default();
        }
        let server = server.trim();
        if !server.contains('=') {
            let endpoint = parse_endpoint(server);
            return Self {
                http: endpoint.clone(),
                https: endpoint,
                bypass: None,
            };
        }
        let mut proxies = Self::default();
        for entry in server.split(';').filter(|entry| !entry.trim().is_empty()) {
            let Some((scheme, endpoint)) = entry.trim().split_once('=') else {
                return Self::default();
            };
            let target = match scheme.trim().to_ascii_lowercase().as_str() {
                "http" => &mut proxies.http,
                "https" => &mut proxies.https,
                _ => continue,
            };
            if target.is_some() {
                return Self::default();
            }
            let Some(endpoint) = parse_endpoint(endpoint.trim()) else {
                return Self::default();
            };
            *target = Some(endpoint);
        }
        proxies
    }

    pub(crate) fn current() -> Self {
        #[cfg(windows)]
        {
            let (enabled, server, bypass) = windows_manual_settings();
            let mut proxies = Self::from_windows_manual_settings(enabled, &server);
            proxies.bypass = normalize_bypass(&bypass);
            proxies
        }
        #[cfg(not(windows))]
        {
            Self::default()
        }
    }
}

fn parse_endpoint(value: &str) -> Option<Url> {
    if value.is_empty() || value.chars().any(char::is_whitespace) {
        return None;
    }
    let url = if value.contains("://") {
        Url::parse(value).ok()?
    } else {
        Url::parse(&format!("http://{value}")).ok()?
    };
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

fn normalize_bypass(value: &str) -> Option<String> {
    if value.len() > 4096 || value.bytes().any(|byte| byte.is_ascii_control()) {
        return None;
    }
    let mut entries = Vec::new();
    for entry in value
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        if entry.eq_ignore_ascii_case("<local>") {
            entries.extend([
                "localhost".to_owned(),
                "127.0.0.0/8".to_owned(),
                "::1".to_owned(),
            ]);
        } else if let Some(domain) = entry.strip_prefix("*.") {
            if domain
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
            {
                entries.push(format!(".{domain}"));
            }
        } else if let Some(prefix) = entry.strip_suffix(".*") {
            let octets = prefix.split('.').collect::<Vec<_>>();
            if (1..=3).contains(&octets.len())
                && octets.iter().all(|part| part.parse::<u8>().is_ok())
            {
                let mut address = octets.join(".");
                for _ in octets.len()..4 {
                    address.push_str(".0");
                }
                entries.push(format!("{address}/{}", octets.len() * 8));
            }
        } else if entry == "*"
            || entry.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'/')
            })
        {
            entries.push(entry.to_owned());
        }
    }
    (!entries.is_empty()).then(|| entries.join(","))
}

#[cfg(windows)]
fn windows_manual_settings() -> (bool, String, String) {
    use std::{mem::size_of, ptr};
    use windows_sys::Win32::{
        Foundation::ERROR_SUCCESS,
        System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ},
    };

    let key = "Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\0"
        .encode_utf16()
        .collect::<Vec<_>>();
    let enabled_name = "ProxyEnable\0".encode_utf16().collect::<Vec<_>>();
    let mut enabled = 0u32;
    let mut enabled_size = size_of::<u32>() as u32;
    // All pointers reference live, correctly aligned, bounded buffers; this is
    // a read of the current user's existing Internet Settings, never a write.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            enabled_name.as_ptr(),
            RRF_RT_REG_DWORD,
            ptr::null_mut(),
            (&mut enabled as *mut u32).cast(),
            &mut enabled_size,
        )
    };
    if status != ERROR_SUCCESS || enabled != 1 {
        return (false, String::new(), String::new());
    }
    let read_string = |name: &str| {
        let name = name.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        let mut buffer = [0u16; 4097];
        let mut bytes = (buffer.len() * size_of::<u16>()) as u32;
        // REG_SZ reads are confined to the fixed Internet Settings key and
        // the zero-initialized UTF-16 buffer above, including termination.
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut bytes,
            )
        };
        if status != ERROR_SUCCESS
            || bytes as usize > buffer.len() * size_of::<u16>()
            || !bytes.is_multiple_of(2)
        {
            return String::new();
        }
        let units = bytes as usize / 2;
        let end = buffer[..units]
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units);
        String::from_utf16(&buffer[..end]).unwrap_or_default()
    };
    (
        true,
        read_string("ProxyServer"),
        read_string("ProxyOverride"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_shared_windows_proxy_routes_both_protocols() {
        let proxies = SystemProxies::from_windows_manual_settings(true, "127.0.0.1:7897");
        assert_eq!(
            proxies.http.as_ref().map(Url::as_str),
            Some("http://127.0.0.1:7897/")
        );
        assert_eq!(
            proxies.https.as_ref().map(Url::as_str),
            Some("http://127.0.0.1:7897/")
        );
    }

    #[test]
    fn protocol_specific_windows_proxy_keeps_separate_http_and_https_routes() {
        let proxies = SystemProxies::from_windows_manual_settings(
            true,
            "http=127.0.0.1:8080;https=127.0.0.1:8443",
        );
        assert_eq!(
            proxies.http.as_ref().map(Url::as_str),
            Some("http://127.0.0.1:8080/")
        );
        assert_eq!(
            proxies.https.as_ref().map(Url::as_str),
            Some("http://127.0.0.1:8443/")
        );
    }

    #[test]
    fn disabled_or_unsafe_windows_proxy_never_injects_a_route() {
        for (enabled, server) in [
            (false, "127.0.0.1:7897"),
            (true, "http://user:password@proxy.test:8080"),
            (true, "http://proxy.test:8080/path"),
            (true, "http://proxy.test:8080/?token=secret"),
            (true, "http://proxy.test:8080/#fragment"),
            (true, "file:///C:/private"),
            (true, "127.0.0.1:7897\r\nInjected: yes"),
        ] {
            let proxies = SystemProxies::from_windows_manual_settings(enabled, server);
            assert!(proxies.http.is_none());
            assert!(proxies.https.is_none());
        }
    }

    #[test]
    fn windows_bypass_entries_preserve_common_domain_and_local_exclusions() {
        assert_eq!(
            normalize_bypass("<local>;*.example.test;192.168.*"),
            Some("localhost,127.0.0.0/8,::1,.example.test,192.168.0.0/16".to_owned())
        );
        assert_eq!(normalize_bypass("example.test\r\nInjected: yes"), None);
    }
}
