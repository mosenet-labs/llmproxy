use crate::{StoreError, StoreResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpaceKind {
    Personal,
    Organization,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OrganizationRole {
    Owner,
    Admin,
    Member,
}
impl OrganizationRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Admin => "admin",
            Self::Member => "member",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Owner => "所有者",
            Self::Admin => "管理员",
            Self::Member => "成员",
        }
    }
    pub fn can_manage(self) -> bool {
        self != Self::Member
    }
    pub(crate) fn parse(s: &str) -> StoreResult<Self> {
        match s {
            "owner" => Ok(Self::Owner),
            "admin" => Ok(Self::Admin),
            "member" => Ok(Self::Member),
            _ => Err(StoreError::Validation("无效的组织角色".into())),
        }
    }
}
#[derive(Clone, Debug)]
pub struct ResourceSpace {
    pub id: i64,
    pub name: String,
    pub kind: SpaceKind,
    pub enabled: bool,
    pub role: OrganizationRole,
    pub version: u64,
}
#[derive(Clone, Debug)]
pub struct OrganizationMember {
    pub id: i64,
    pub user_id: i64,
    pub email: String,
    pub enabled: bool,
    pub role: OrganizationRole,
    pub group_ids: Vec<i64>,
    pub version: u64,
}
#[derive(Clone, Debug)]
pub struct OrganizationInvitation {
    pub id: i64,
    pub space_id: i64,
    pub space_name: String,
    pub email: String,
    pub role: OrganizationRole,
    pub expires_at: i64,
    pub version: u64,
}
