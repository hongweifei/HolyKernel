//! 标签与标签成员关系。

use alloc::string::String;
use alloc::vec::Vec;

use hk_block::BlockDevice;

use crate::error::HolyError;
use crate::ids::DocId;
use crate::types::validate_name;

use super::Volume;

impl<D: BlockDevice> Volume<D> {
    /// 创建标签。
    pub(crate) fn create_tag(&mut self, name: &str) -> Result<u32, HolyError> {
        validate_name(name)?;
        self.check_quota(0, 0, 0, 0, 1)?;
        if self.tags.values().any(|existing| existing == name) {
            return Err(HolyError::Exists);
        }
        let tag = self.next_tag;
        self.next_tag = self.next_tag.saturating_add(1);
        self.tags.insert(tag, String::from(name));
        self.dirty = true;
        Ok(tag)
    }

    /// 删除标签及其成员关系。
    pub(crate) fn drop_tag(&mut self, tag: u32) -> Result<(), HolyError> {
        if self.tags.remove(&tag).is_none() {
            return Err(HolyError::NotFound);
        }
        self.tag_members.remove(&tag);
        self.dirty = true;
        Ok(())
    }

    /// 把文档加入标签。
    pub(crate) fn tag(&mut self, document: DocId, tag: u32) -> Result<(), HolyError> {
        if !self.documents.contains_key(&document) {
            return Err(HolyError::NotFound);
        }
        if !self.tags.contains_key(&tag) {
            return Err(HolyError::NotFound);
        }
        self.tag_members.entry(tag).or_default().insert(document);
        self.dirty = true;
        Ok(())
    }

    /// 把文档从标签移除。
    pub(crate) fn untag(&mut self, document: DocId, tag: u32) -> Result<(), HolyError> {
        if !self.tags.contains_key(&tag) {
            return Err(HolyError::NotFound);
        }
        if let Some(members) = self.tag_members.get_mut(&tag) {
            members.remove(&document);
        }
        self.dirty = true;
        Ok(())
    }

    /// 枚举标签。
    pub(crate) fn tags(&self) -> Vec<hk_fs::TagInfo> {
        self.tags
            .iter()
            .map(|(id, name)| hk_fs::TagInfo {
                id: *id,
                name: name.clone(),
            })
            .collect()
    }

    /// 枚举标签成员。
    pub(crate) fn tag_members(&self, tag: u32) -> Result<Vec<DocId>, HolyError> {
        if !self.tags.contains_key(&tag) {
            return Err(HolyError::NotFound);
        }
        Ok(self
            .tag_members
            .get(&tag)
            .map(|members| members.iter().copied().collect())
            .unwrap_or_default())
    }

    /// 枚举文档所属标签。
    pub(crate) fn tags_of(&self, document: DocId) -> Result<Vec<u32>, HolyError> {
        if !self.documents.contains_key(&document) {
            return Err(HolyError::NotFound);
        }
        Ok(self
            .tag_members
            .iter()
            .filter(|(_, members)| members.contains(&document))
            .map(|(tag, _)| *tag)
            .collect())
    }
}
