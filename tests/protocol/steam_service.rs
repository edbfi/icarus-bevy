use icarus_bevy::steam_service::{AuthClient, ServiceLimits};
use std::time::Duration;

#[test]
fn client_is_shareable_without_exposing_credentials() {
    fn shareable<T: Clone + Send + Sync>() {}
    shareable::<AuthClient>();
}

#[test]
fn service_limits_bound_queues_work_and_auth_lifetimes() {
    ServiceLimits::default().validate().unwrap();
    for limits in [
        ServiceLimits {
            queue_capacity: 0,
            ..ServiceLimits::default()
        },
        ServiceLimits {
            queue_capacity: 4097,
            ..ServiceLimits::default()
        },
        ServiceLimits {
            max_active: 65,
            ..ServiceLimits::default()
        },
        ServiceLimits {
            max_requests: 4097,
            ..ServiceLimits::default()
        },
        ServiceLimits {
            auth_timeout: Duration::ZERO,
            ..ServiceLimits::default()
        },
        ServiceLimits {
            poll_interval: Duration::ZERO,
            ..ServiceLimits::default()
        },
        ServiceLimits {
            poll_interval: Duration::from_secs(2),
            ..ServiceLimits::default()
        },
        ServiceLimits {
            presence_refresh: Duration::ZERO,
            ..ServiceLimits::default()
        },
        ServiceLimits {
            max_packets_per_poll: 0,
            ..ServiceLimits::default()
        },
        ServiceLimits {
            max_packets_per_poll: 1025,
            ..ServiceLimits::default()
        },
    ] {
        assert!(limits.validate().is_err());
    }
}
