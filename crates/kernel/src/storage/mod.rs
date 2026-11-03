//! Host-only storage primitives; authorization and durable jobs live above these
//! codecs and adapters. Backup transfers require stopped, fenced sources and an
//! unpublished destination owned by the caller.
pub mod xfs;

pub mod blob;

pub mod backup_crypto;

pub mod s3;

pub mod archive;

pub mod backup_transfer;

pub mod metadata_snapshot;
