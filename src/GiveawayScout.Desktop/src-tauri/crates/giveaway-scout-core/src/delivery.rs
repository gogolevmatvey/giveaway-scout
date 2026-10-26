//! Delivery policy independent of the native notification transport.
use std::sync::{Mutex, MutexGuard};

use thiserror::Error;

use crate::{AppSettings, Database, NotificationRecord, StorageError};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DeliveryReport {
    pub attempted: u64,
    pub delivered: u64,
    pub failed: u64,
    pub skipped: u64,
}

#[derive(Debug, Error)]
pub enum DeliveryError {
    #[error("database lock is unavailable")]
    DatabaseLockUnavailable,
    #[error(transparent)]
    Storage(#[from] StorageError),
}

fn lock(database: &Mutex<Database>) -> Result<MutexGuard<'_, Database>, DeliveryError> {
    database
        .lock()
        .map_err(|_| DeliveryError::DatabaseLockUnavailable)
}

/// The application must serialize delivery/checks and cleanup. Each selected
/// record is revalidated against current settings/read/delivery state before
/// its attempt begins. No database mutex is held during native transport or
/// activation callbacks. An already-started OS request cannot be recalled here.
pub fn dispatch_pending_notifications(
    database: &Mutex<Database>,
    mut deliver: impl FnMut(&NotificationRecord, &AppSettings) -> Result<(), String>,
) -> Result<DeliveryReport, DeliveryError> {
    let ids: Vec<_> = lock(database)?
        .pending_notifications()?
        .into_iter()
        .map(|notification| notification.id)
        .collect();
    let mut report = DeliveryReport::default();
    for id in ids {
        let (notification, settings) = {
            let database = lock(database)?;
            let Some(notification) = database.notification_for_delivery(&id)? else {
                report.skipped += 1;
                continue;
            };
            (notification, database.settings()?)
        };
        report.attempted += 1;
        let result = deliver(&notification, &settings);
        let database = lock(database)?;
        match result {
            Ok(()) => {
                database.record_notification_delivered(&id)?;
                report.delivered += 1;
            }
            Err(error) => {
                database.record_notification_delivery_failure(&id, &error)?;
                log::warn!("notification {id} delivery failed: {error}");
                report.failed += 1;
            }
        }
    }
    Ok(report)
}
