//! Persistence: the app's SQLite database (session history + message
//! transcripts) and the rows it stores.

mod db;

pub use db::{Db, DbError, MessageRow, SessionRow};
