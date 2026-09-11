use alloc::string::String;
use alloc::vec::Vec;
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClientContext {
    pub package_name: String,
    pub permissions: Vec<String>,
    pub permission_values: Vec<PermissionValueGrant>,
    pub is_foreground: bool,
    pub user_id: Option<u64>,
}

impl ClientContext {
    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions
            .iter()
            .any(|candidate| candidate == permission)
    }

    pub fn granted_values(&self, permission: &str) -> Vec<String> {
        self.permission_values
            .iter()
            .find(|grant| grant.permission == permission)
            .map(|grant| grant.values.clone())
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionValueGrant {
    pub permission: String,
    pub values: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FidlCapability<'a> {
    pub protocol: &'a str,
    pub capability: &'a str,
    pub permission: Option<&'a str>,
    pub method_ordinals: &'a [u64],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PermissionDecision {
    Allow,
    Deny,
}

pub fn check_permission(permission: Option<&str>, client: &ClientContext) -> PermissionDecision {
    let Some(permission) = permission.map(str::trim).filter(|value| !value.is_empty()) else {
        return PermissionDecision::Allow;
    };

    // Preference management is reserved for the platform broker/debugger.
    // Ordinary signed applications cannot acquire it by declaring the name.
    if permission == "bexos.permission.MANAGE_USER_PREFERENCES"
        && !matches!(
            client.package_name.as_str(),
            "bexos.platform.appd" | "bexos.driver.debugd"
        )
    {
        return PermissionDecision::Deny;
    }
    if is_plain_permission(permission) {
        return if client.has_permission(permission) {
            PermissionDecision::Allow
        } else {
            PermissionDecision::Deny
        };
    }

    for clause in permission.split("&&").map(str::trim) {
        if let Some(required) = parse_contains_clause(clause) {
            if !client.has_permission(required) {
                return PermissionDecision::Deny;
            }
        } else if clause == "client.is_foreground" {
            if !client.is_foreground {
                return PermissionDecision::Deny;
            }
        } else {
            return PermissionDecision::Deny;
        }
    }

    PermissionDecision::Allow
}

pub fn allowed_capabilities<'a>(
    capabilities: &'a [FidlCapability<'a>],
    client: &ClientContext,
) -> Vec<FidlCapability<'a>> {
    capabilities
        .iter()
        .copied()
        .filter(|capability| {
            check_permission(capability.permission, client) == PermissionDecision::Allow
        })
        .collect()
}

pub fn permission_scope_name(permission: Option<&str>) -> Option<&str> {
    let permission = permission?.trim();
    if permission.is_empty() {
        return None;
    }
    if is_plain_permission(permission) {
        return Some(permission);
    }
    permission
        .split("&&")
        .map(str::trim)
        .find_map(parse_contains_clause)
}

fn is_plain_permission(permission: &str) -> bool {
    permission
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | ':' | '-' | '='))
}

fn parse_contains_clause(clause: &str) -> Option<&str> {
    const PREFIX: &str = "request.permissions.contains(";
    let inner = clause.strip_prefix(PREFIX)?.strip_suffix(')')?.trim();

    parse_quoted(inner, '\'').or_else(|| parse_quoted(inner, '"'))
}

fn parse_quoted(value: &str, quote: char) -> Option<&str> {
    let value = value.strip_prefix(quote)?.strip_suffix(quote)?;
    if value.is_empty() { None } else { Some(value) }
}
