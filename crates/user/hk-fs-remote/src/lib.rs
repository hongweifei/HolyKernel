//! 通过 IPC 访问 Holy 文档服务的客户端后端。
//!
//! VFS 与 Path Server 都把远程 Holy 服务当作普通 [`hk_fs::Backend`]；
//! 服务名、盘符与端点解析都封装在这里。

#![no_std]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use hk_fs::{
    Backend, Caps, DocId, DocumentInfo, DocumentKind, EntryInfo, Error, RevisionFlags, RevisionId,
    RevisionInfo,
};
use hk_proto::holy::{
    DocId as WireDocId, DocumentKind as WireKind, KIND_COLLECTION, RevisionId as WireRevisionId,
    VolumeView, decode_entries, decode_revisions,
};
use hk_proto_client::HolyClient;

/// 通过 IPC 访问的远程文档后端。
pub struct RemoteHolyFs {
    root: DocId,
    volume: VolumeView,
    drive: u8,
    client: RefCell<HolyClient>,
}

impl RemoteHolyFs {
    /// 连接指定服务并读取盘符的根文档与卷视图。
    pub fn new(own_endpoint: u64, drive: u8, service: &'static str) -> Result<Self, Error> {
        let mut client = HolyClient::new_with_service(own_endpoint, service);
        client.set_drive(drive);
        let root = client.root().map_err(Error::from_status_code)?;
        let volume = client.volume().map_err(Error::from_status_code)?;
        Ok(Self {
            root: DocId::new(root.get()),
            volume,
            drive,
            client: RefCell::new(client),
        })
    }

    /// 返回盘符的卷视图。
    pub fn volume(&self) -> VolumeView {
        self.volume
    }
}

impl Backend for RemoteHolyFs {
    fn root(&self) -> DocId {
        self.root
    }

    fn caps(&self) -> Caps {
        Caps::NATIVE
    }

    fn cacheable(&self, _document: DocId) -> bool {
        self.drive != b'D' && self.drive != b'P'
    }

    fn document(&self, document: DocId) -> Result<DocumentInfo, Error> {
        let view = self
            .client
            .borrow_mut()
            .document_id(wire_doc(document))
            .map_err(Error::from_status_code)?;
        Ok(DocumentInfo {
            id: document,
            kind: kind_of(view.kind),
            head: RevisionId::new(view.head),
            parent: (view.parent != 0).then(|| RevisionId::new(view.parent)),
            size: view.size,
            moment: view.moment,
        })
    }

    fn resolve(&self, collection: DocId, name: &str) -> Result<DocId, Error> {
        let document = self
            .client
            .borrow_mut()
            .resolve(wire_doc(collection), name)
            .map_err(Error::from_status_code)?;
        Ok(native_doc(document))
    }

    fn enumerate(&self, collection: DocId) -> Result<Vec<EntryInfo>, Error> {
        let mut client = self.client.borrow_mut();
        let bytes = client
            .enumerate_id(wire_doc(collection))
            .map_err(Error::from_status_code)?;
        Ok(decode_entries(bytes)
            .map(|entry| EntryInfo {
                name: String::from(entry.name),
                document: DocId::new(entry.document),
                kind: kind_of(entry.kind),
            })
            .collect())
    }

    fn create(
        &mut self,
        collection: DocId,
        name: &str,
        kind: DocumentKind,
    ) -> Result<DocId, Error> {
        let document = self
            .client
            .borrow_mut()
            .create(wire_doc(collection), name, wire_kind(kind))
            .map_err(Error::from_status_code)?;
        Ok(native_doc(document))
    }

    fn unbind(&mut self, collection: DocId, name: &str) -> Result<DocId, Error> {
        let document = self
            .client
            .borrow_mut()
            .unbind(wire_doc(collection), name)
            .map_err(Error::from_status_code)?;
        Ok(native_doc(document))
    }

    fn read(
        &self,
        document: DocId,
        revision: Option<RevisionId>,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, Error> {
        let mut client = self.client.borrow_mut();
        match revision {
            Some(revision) => client
                .read_revision(
                    wire_doc(document),
                    WireRevisionId::new(revision.get()),
                    offset,
                    buffer,
                )
                .map_err(Error::from_status_code),
            None => client
                .read_id(wire_doc(document), offset, buffer)
                .map_err(Error::from_status_code),
        }
    }

    fn write(&mut self, document: DocId, offset: u64, data: &[u8]) -> Result<usize, Error> {
        self.client
            .borrow_mut()
            .write_id(wire_doc(document), offset, data)
            .map_err(Error::from_status_code)?;
        Ok(data.len())
    }

    fn truncate(&mut self, document: DocId, size: u64) -> Result<(), Error> {
        self.client
            .borrow_mut()
            .truncate_id(wire_doc(document), size)
            .map_err(Error::from_status_code)
    }

    fn revisions(&self, document: DocId) -> Result<Vec<RevisionInfo>, Error> {
        let mut client = self.client.borrow_mut();
        let bytes = client
            .revisions_id(wire_doc(document))
            .map_err(Error::from_status_code)?;
        Ok(decode_revisions(bytes)
            .map(|revision| RevisionInfo {
                id: RevisionId::new(revision.id),
                parent: (revision.parent != 0).then(|| RevisionId::new(revision.parent)),
                size: revision.size,
                moment: revision.moment,
                flags: RevisionFlags::from_bits(revision.flags),
            })
            .collect())
    }

    fn sync(&mut self) -> Result<(), Error> {
        self.client
            .borrow_mut()
            .sync()
            .map_err(Error::from_status_code)
    }
}

fn wire_doc(document: DocId) -> WireDocId {
    WireDocId::new(document.get())
}

fn native_doc(document: WireDocId) -> DocId {
    DocId::new(document.get())
}

fn kind_of(code: u8) -> DocumentKind {
    if code == KIND_COLLECTION {
        DocumentKind::Collection
    } else {
        DocumentKind::Content
    }
}

fn wire_kind(kind: DocumentKind) -> WireKind {
    match kind {
        DocumentKind::Content => WireKind::Content,
        DocumentKind::Collection => WireKind::Collection,
    }
}
