use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock, Weak};

use anyhow::Result;
use async_trait::async_trait;

use crate::coordinator::{
    Coordinator, NamespaceOrigin, NamespaceRegistration, TrackInfo, TrackRegistration,
};

/// Internal state shared between LocalCoordinator and its drop guards.
struct LocalCoordinatorState {
    /// Local namespaces currently registered
    namespaces: RwLock<HashSet<String>>,

    /// Tracks registered under each namespace
    /// Maps namespace -> set of track names
    tracks: RwLock<HashMap<String, HashSet<String>>>,
}

/// Drop guard for namespace registration.
/// When dropped, removes the namespace from the coordinator.
struct NamespaceDropGuard {
    namespace: String,
    state: Weak<LocalCoordinatorState>,
}

impl Drop for NamespaceDropGuard {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            log::debug!(
                "local coordinator: auto-unregistering namespace {} (drop guard)",
                self.namespace
            );
            state.namespaces.write().unwrap().remove(&self.namespace);
            state.tracks.write().unwrap().remove(&self.namespace);
        }
    }
}

/// Drop guard for track registration.
/// When dropped, removes the track from the coordinator.
struct TrackDropGuard {
    namespace: String,
    track_name: String,
    state: Weak<LocalCoordinatorState>,
}

impl Drop for TrackDropGuard {
    fn drop(&mut self) {
        if let Some(state) = self.state.upgrade() {
            log::debug!(
                "local coordinator: auto-unregistering track {}/{} (drop guard)",
                self.namespace,
                self.track_name
            );
            if let Some(tracks) = state.tracks.write().unwrap().get_mut(&self.namespace) {
                tracks.remove(&self.track_name);
            }
        }
    }
}

/// Local coordinator for single-relay deployments.
///
/// This coordinator does not communicate with any external service.
/// It tracks local namespaces and tracks in memory but does not
/// advertise them to other relays.
///
/// # Use Cases
///
/// - Development and testing
/// - Single-relay deployments
/// - Environments without external registry
///
/// # Example
///
/// ```rust,ignore
/// use moq_relay_ietf::{Relay, RelayConfig, LocalCoordinator};
///
/// let config = RelayConfig {
///     coordinator: LocalCoordinator::new(),
///     // ...
/// };
/// let relay = Relay::new(config)?;
/// ```
pub struct LocalCoordinator {
    state: Arc<LocalCoordinatorState>,
}

impl LocalCoordinator {
    /// Create a new local coordinator.
    pub fn new() -> Arc<dyn Coordinator> {
        Arc::new(Self {
            state: Arc::new(LocalCoordinatorState {
                namespaces: RwLock::new(HashSet::new()),
                tracks: RwLock::new(HashMap::new()),
            }),
        })
    }

    /// Get the number of registered namespaces (for testing).
    #[cfg(test)]
    pub fn namespace_count(&self) -> usize {
        self.state.namespaces.read().unwrap().len()
    }

    /// Get the number of tracks under a namespace (for testing).
    #[cfg(test)]
    pub fn track_count(&self, namespace: &str) -> usize {
        self.state
            .tracks
            .read()
            .unwrap()
            .get(namespace)
            .map(|t| t.len())
            .unwrap_or(0)
    }

    /// Check if a namespace is registered (for testing).
    #[cfg(test)]
    pub fn has_namespace(&self, namespace: &str) -> bool {
        self.state.namespaces.read().unwrap().contains(namespace)
    }

    /// Check if a track is registered (for testing).
    #[cfg(test)]
    pub fn has_track(&self, namespace: &str, track_name: &str) -> bool {
        self.state
            .tracks
            .read()
            .unwrap()
            .get(namespace)
            .map(|t| t.contains(track_name))
            .unwrap_or(false)
    }
}

impl Default for LocalCoordinator {
    fn default() -> Self {
        Self {
            state: Arc::new(LocalCoordinatorState {
                namespaces: RwLock::new(HashSet::new()),
                tracks: RwLock::new(HashMap::new()),
            }),
        }
    }
}

#[async_trait]
impl Coordinator for LocalCoordinator {
    async fn register_namespace(&self, namespace: &str) -> Result<NamespaceRegistration> {
        log::debug!("local coordinator: registering namespace {}", namespace);

        self.state
            .namespaces
            .write()
            .unwrap()
            .insert(namespace.to_string());

        // Initialize empty track set for this namespace
        self.state
            .tracks
            .write()
            .unwrap()
            .entry(namespace.to_string())
            .or_default();

        // Return a drop guard that will unregister the namespace when dropped
        let guard = NamespaceDropGuard {
            namespace: namespace.to_string(),
            state: Arc::downgrade(&self.state),
        };
        Ok(NamespaceRegistration::new(guard))
    }

    async fn unregister_namespace(&self, namespace: &str) -> Result<()> {
        log::debug!("local coordinator: unregistering namespace {}", namespace);

        self.state.namespaces.write().unwrap().remove(namespace);
        self.state.tracks.write().unwrap().remove(namespace);

        Ok(())
    }

    async fn register_track(&self, track_info: TrackInfo) -> Result<TrackRegistration> {
        log::debug!(
            "local coordinator: registering track {}/{} (alias={})",
            track_info.namespace,
            track_info.track_name,
            track_info.track_alias
        );

        self.state
            .tracks
            .write()
            .unwrap()
            .entry(track_info.namespace.clone())
            .or_default()
            .insert(track_info.track_name.clone());

        // Return a drop guard that will unregister the track when dropped
        let guard = TrackDropGuard {
            namespace: track_info.namespace,
            track_name: track_info.track_name,
            state: Arc::downgrade(&self.state),
        };
        Ok(TrackRegistration::new(guard))
    }

    async fn unregister_track(&self, namespace: &str, track_name: &str) -> Result<()> {
        log::debug!(
            "local coordinator: unregistering track {}/{}",
            namespace,
            track_name
        );

        if let Some(tracks) = self.state.tracks.write().unwrap().get_mut(namespace) {
            tracks.remove(track_name);
        }

        Ok(())
    }

    async fn lookup(&self, namespace: &str) -> Result<Option<NamespaceOrigin>> {
        // Check if we have this namespace locally
        if self.state.namespaces.read().unwrap().contains(namespace) {
            log::debug!("local coordinator: lookup {} -> Local", namespace);
            return Ok(Some(NamespaceOrigin::Local));
        }

        // Single node - if not local, it doesn't exist
        log::debug!("local coordinator: lookup {} -> not found", namespace);
        Ok(None)
    }

    async fn shutdown(&self) -> Result<()> {
        log::debug!("local coordinator: shutdown");

        self.state.namespaces.write().unwrap().clear();
        self.state.tracks.write().unwrap().clear();

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_coordinator() -> Arc<LocalCoordinator> {
        Arc::new(LocalCoordinator::default())
    }

    #[tokio::test]
    async fn test_register_namespace() {
        let coord = create_coordinator();

        let _reg = coord.register_namespace("live/stream").await.unwrap();

        assert!(coord.has_namespace("live/stream"));
        assert_eq!(coord.namespace_count(), 1);
    }

    #[tokio::test]
    async fn test_unregister_namespace() {
        let coord = create_coordinator();

        let _reg = coord.register_namespace("live/stream").await.unwrap();
        assert!(coord.has_namespace("live/stream"));

        coord.unregister_namespace("live/stream").await.unwrap();
        assert!(!coord.has_namespace("live/stream"));
        assert_eq!(coord.namespace_count(), 0);
    }

    #[tokio::test]
    async fn test_register_track() {
        let coord = create_coordinator();

        let _ns_reg = coord.register_namespace("live/stream").await.unwrap();

        let track_info = TrackInfo {
            namespace: "live/stream".to_string(),
            track_name: "audio".to_string(),
            track_alias: 1,
        };
        let _track_reg = coord.register_track(track_info).await.unwrap();

        assert!(coord.has_track("live/stream", "audio"));
        assert_eq!(coord.track_count("live/stream"), 1);
    }

    #[tokio::test]
    async fn test_unregister_track_keeps_namespace() {
        let coord = create_coordinator();

        let _ns_reg = coord.register_namespace("live/stream").await.unwrap();

        let track_info = TrackInfo {
            namespace: "live/stream".to_string(),
            track_name: "audio".to_string(),
            track_alias: 1,
        };
        let _track_reg = coord.register_track(track_info).await.unwrap();

        // Unregister track explicitly
        coord
            .unregister_track("live/stream", "audio")
            .await
            .unwrap();

        // Track should be gone but namespace should remain
        assert!(!coord.has_track("live/stream", "audio"));
        assert!(coord.has_namespace("live/stream"));
    }

    #[tokio::test]
    async fn test_unregister_namespace_removes_tracks() {
        let coord = create_coordinator();

        let _ns_reg = coord.register_namespace("live/stream").await.unwrap();

        let track_info = TrackInfo {
            namespace: "live/stream".to_string(),
            track_name: "audio".to_string(),
            track_alias: 1,
        };
        let _track_reg = coord.register_track(track_info).await.unwrap();

        // Unregister namespace explicitly
        coord.unregister_namespace("live/stream").await.unwrap();

        // Both namespace and tracks should be gone
        assert!(!coord.has_namespace("live/stream"));
        assert_eq!(coord.track_count("live/stream"), 0);
    }

    #[tokio::test]
    async fn test_lookup_local() {
        let coord = create_coordinator();

        let _reg = coord.register_namespace("live/stream").await.unwrap();

        let result = coord.lookup("live/stream").await.unwrap();
        assert_eq!(result, Some(NamespaceOrigin::Local));
    }

    #[tokio::test]
    async fn test_lookup_not_found() {
        let coord = create_coordinator();

        let result = coord.lookup("unknown/stream").await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_shutdown_clears_all() {
        let coord = create_coordinator();

        let _ns1_reg = coord.register_namespace("ns1").await.unwrap();
        let _ns2_reg = coord.register_namespace("ns2").await.unwrap();

        let track_info = TrackInfo {
            namespace: "ns1".to_string(),
            track_name: "track1".to_string(),
            track_alias: 1,
        };
        let _track_reg = coord.register_track(track_info).await.unwrap();

        coord.shutdown().await.unwrap();

        assert_eq!(coord.namespace_count(), 0);
        assert_eq!(coord.track_count("ns1"), 0);
    }

    #[tokio::test]
    async fn test_multiple_tracks_per_namespace() {
        let coord = create_coordinator();

        let _ns_reg = coord.register_namespace("live/stream").await.unwrap();

        let mut _track_regs = Vec::new();
        for (i, name) in ["audio", "video", "data"].iter().enumerate() {
            let track_info = TrackInfo {
                namespace: "live/stream".to_string(),
                track_name: name.to_string(),
                track_alias: i as u64,
            };
            _track_regs.push(coord.register_track(track_info).await.unwrap());
        }

        assert_eq!(coord.track_count("live/stream"), 3);
        assert!(coord.has_track("live/stream", "audio"));
        assert!(coord.has_track("live/stream", "video"));
        assert!(coord.has_track("live/stream", "data"));
    }

    #[tokio::test]
    async fn test_drop_unregisters_namespace() {
        let coord = create_coordinator();

        {
            let _reg = coord.register_namespace("live/stream").await.unwrap();
            assert!(coord.has_namespace("live/stream"));
            // _reg dropped here
        }

        // Namespace should be auto-unregistered
        assert!(!coord.has_namespace("live/stream"));
        assert_eq!(coord.namespace_count(), 0);
    }

    #[tokio::test]
    async fn test_drop_unregisters_track() {
        let coord = create_coordinator();

        let _ns_reg = coord.register_namespace("live/stream").await.unwrap();

        {
            let track_info = TrackInfo {
                namespace: "live/stream".to_string(),
                track_name: "audio".to_string(),
                track_alias: 1,
            };
            let _track_reg = coord.register_track(track_info).await.unwrap();
            assert!(coord.has_track("live/stream", "audio"));
            // _track_reg dropped here
        }

        // Track should be auto-unregistered, but namespace remains
        assert!(!coord.has_track("live/stream", "audio"));
        assert!(coord.has_namespace("live/stream"));
    }
}
