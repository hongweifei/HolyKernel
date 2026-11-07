//! 把路径形状的后端适配为 Holy 对象后端。

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::cell::RefCell;

use hk_fs::{
    Backend, Caps, DocId, DocumentInfo, DocumentKind, EntryInfo, Error, RevisionFlags, RevisionId,
    RevisionInfo,
};

use crate::vfs::{FileType, NodeId, PathBackend, VfsError};

struct State {
    root: NodeId,
    nodes: BTreeMap<NodeId, DocId>,
    documents: BTreeMap<DocId, NodeId>,
    revisions: BTreeMap<DocId, RevisionId>,
    next_document: u128,
    next_revision: u64,
}

impl State {
    fn new(root: NodeId) -> Self {
        let mut nodes = BTreeMap::new();
        nodes.insert(root, DocId::ROOT);
        let mut documents = BTreeMap::new();
        documents.insert(DocId::ROOT, root);
        let mut revisions = BTreeMap::new();
        revisions.insert(DocId::ROOT, RevisionId::ROOT);
        Self {
            root,
            nodes,
            documents,
            revisions,
            next_document: DocId::ROOT.get() + 1,
            next_revision: RevisionId::ROOT.get() + 1,
        }
    }

    fn document_for(&mut self, node: NodeId) -> DocId {
        if let Some(document) = self.nodes.get(&node) {
            return *document;
        }
        let document = DocId::new(self.next_document);
        self.next_document = self.next_document.saturating_add(1);
        self.nodes.insert(node, document);
        self.documents.insert(document, node);
        document
    }

    fn node_for(&self, document: DocId) -> Result<NodeId, Error> {
        self.documents
            .get(&document)
            .copied()
            .ok_or(Error::NotFound)
    }

    fn revision(&self, document: DocId) -> RevisionId {
        self.revisions
            .get(&document)
            .copied()
            .unwrap_or(RevisionId::ROOT)
    }

    fn bump(&mut self, document: DocId) -> RevisionId {
        let revision = RevisionId::new(self.next_revision);
        self.next_revision = self.next_revision.saturating_add(1);
        self.revisions.insert(document, revision);
        revision
    }
}

/// 路径后端到 Holy 后端的适配器。
pub struct PathObject<P: PathBackend> {
    backend: RefCell<P>,
    state: RefCell<State>,
}

impl<P: PathBackend> PathObject<P> {
    /// 包装路径后端。
    pub fn new(backend: P) -> Self {
        let root = backend.root();
        Self {
            backend: RefCell::new(backend),
            state: RefCell::new(State::new(root)),
        }
    }

    /// 取回路径后端。
    pub fn into_inner(self) -> P {
        self.backend.into_inner()
    }

    fn document_for_node(&self, node: NodeId) -> DocId {
        self.state.borrow_mut().document_for(node)
    }

    fn node_for(&self, document: DocId) -> Result<NodeId, Error> {
        self.state.borrow().node_for(document)
    }
}

impl<P: PathBackend> Backend for PathObject<P> {
    fn root(&self) -> DocId {
        let root = self.state.borrow().root;
        self.document_for_node(root)
    }

    fn caps(&self) -> Caps {
        Caps::PATH
    }

    fn document(&self, document: DocId) -> Result<DocumentInfo, Error> {
        let node = self.node_for(document)?;
        let metadata = self.backend.borrow().metadata(node).map_err(map_error)?;
        Ok(DocumentInfo {
            id: document,
            kind: kind(metadata.kind),
            head: self.state.borrow().revision(document),
            parent: None,
            size: metadata.size,
            moment: 0,
        })
    }

    fn resolve(&self, collection: DocId, name: &str) -> Result<DocId, Error> {
        let node = self.node_for(collection)?;
        let node = self
            .backend
            .borrow()
            .lookup(node, name)
            .map_err(map_error)?;
        Ok(self.document_for_node(node))
    }

    fn enumerate(&self, collection: DocId) -> Result<Vec<EntryInfo>, Error> {
        let node = self.node_for(collection)?;
        let entries = self.backend.borrow().readdir(node).map_err(map_error)?;
        let mut result = Vec::with_capacity(entries.len());
        for entry in entries {
            let child = self.lookup_node(node, &entry.name).unwrap_or(node);
            result.push(EntryInfo {
                name: entry.name,
                document: self.document_for_node(child),
                kind: kind(entry.kind),
            });
        }
        Ok(result)
    }

    fn create(
        &mut self,
        collection: DocId,
        name: &str,
        kind: DocumentKind,
    ) -> Result<DocId, Error> {
        let node = self.node_for(collection)?;
        let node = self
            .backend
            .get_mut()
            .create(node, name, file_type(kind))
            .map_err(map_error)?;
        Ok(self.document_for_node(node))
    }

    fn unbind(&mut self, collection: DocId, name: &str) -> Result<DocId, Error> {
        let node = self.node_for(collection)?;
        let node = self
            .backend
            .get_mut()
            .unlink(node, name)
            .map_err(map_error)?;
        Ok(self.document_for_node(node))
    }

    fn read(
        &self,
        document: DocId,
        revision: Option<RevisionId>,
        offset: u64,
        buffer: &mut [u8],
    ) -> Result<usize, Error> {
        if let Some(revision) = revision
            && revision != self.state.borrow().revision(document)
        {
            return Err(Error::Unsupported);
        }
        let node = self.node_for(document)?;
        self.backend
            .borrow()
            .read(node, offset, buffer)
            .map_err(map_error)
    }

    fn write(&mut self, document: DocId, offset: u64, data: &[u8]) -> Result<usize, Error> {
        let node = self.node_for(document)?;
        let written = self
            .backend
            .get_mut()
            .write(node, offset, data)
            .map_err(map_error)?;
        self.state.borrow_mut().bump(document);
        Ok(written)
    }

    fn truncate(&mut self, document: DocId, size: u64) -> Result<(), Error> {
        let node = self.node_for(document)?;
        self.backend
            .get_mut()
            .truncate(node, size)
            .map_err(map_error)?;
        self.state.borrow_mut().bump(document);
        Ok(())
    }

    fn revisions(&self, document: DocId) -> Result<Vec<RevisionInfo>, Error> {
        let info = self.document(document)?;
        Ok(alloc::vec![RevisionInfo {
            id: info.head,
            parent: None,
            size: info.size,
            moment: info.moment,
            flags: RevisionFlags::NONE,
        }])
    }

    fn sync(&mut self) -> Result<(), Error> {
        self.backend.get_mut().sync().map_err(map_error)
    }

    fn cacheable(&self, document: DocId) -> bool {
        match self.node_for(document) {
            Ok(node) => self.backend.borrow().cacheable(node),
            Err(_) => false,
        }
    }
}

impl<P: PathBackend> PathObject<P> {
    fn lookup_node(&self, parent: NodeId, name: &str) -> Result<NodeId, VfsError> {
        self.backend.borrow().lookup(parent, name)
    }
}

fn kind(kind: FileType) -> DocumentKind {
    match kind {
        FileType::File => DocumentKind::Content,
        FileType::Directory => DocumentKind::Collection,
    }
}

fn file_type(kind: DocumentKind) -> FileType {
    match kind {
        DocumentKind::Content => FileType::File,
        DocumentKind::Collection => FileType::Directory,
    }
}

fn map_error(error: VfsError) -> Error {
    match error {
        VfsError::NotFound => Error::NotFound,
        VfsError::NotDirectory => Error::NotDirectory,
        VfsError::IsDirectory => Error::IsDirectory,
        VfsError::AlreadyExists => Error::Exists,
        VfsError::PermissionDenied => Error::Refused,
        VfsError::ReadOnly => Error::ReadOnly,
        VfsError::InvalidPath | VfsError::InvalidOffset => Error::Invalid,
        VfsError::Backend => Error::Io,
    }
}
