use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

pub(crate) struct PerIpLimiter {
    inner: Mutex<HashMap<IpAddr, u32>>,
    cap: u32,
}

pub(crate) struct ConnPermit {
    limiter: Arc<PerIpLimiter>,
    ip: IpAddr,
}

impl PerIpLimiter {
    pub(crate) fn new(cap: u32) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            cap,
        }
    }

    pub(crate) fn try_acquire(self: &Arc<Self>, ip: IpAddr) -> Option<ConnPermit> {
        let mut counts = self.inner.lock().unwrap_or_else(|err| err.into_inner());
        let count = counts.entry(ip).or_insert(0);
        if *count >= self.cap {
            return None;
        }
        *count += 1;
        drop(counts);

        Some(ConnPermit {
            limiter: Arc::clone(self),
            ip,
        })
    }
}

impl Drop for ConnPermit {
    fn drop(&mut self) {
        let mut counts = self
            .limiter
            .inner
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let Some(count) = counts.get_mut(&self.ip) else {
            debug_assert!(false, "permit must have a matching limiter entry");
            return;
        };
        *count -= 1;
        if *count == 0 {
            counts.remove(&self.ip);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn v4() -> IpAddr {
        Ipv4Addr::LOCALHOST.into()
    }

    #[test]
    fn acquires_up_to_cap_and_refuses_the_next_permit() {
        let limiter = Arc::new(PerIpLimiter::new(2));

        let first = limiter.try_acquire(v4()).expect("first permit");
        let second = limiter.try_acquire(v4()).expect("second permit");
        assert!(limiter.try_acquire(v4()).is_none());

        drop((first, second));
    }

    #[test]
    fn dropping_a_permit_frees_exactly_one_slot() {
        let limiter = Arc::new(PerIpLimiter::new(2));
        let first = limiter.try_acquire(v4()).expect("first permit");
        let _second = limiter.try_acquire(v4()).expect("second permit");

        drop(first);
        let _replacement = limiter.try_acquire(v4()).expect("replacement permit");
        assert!(limiter.try_acquire(v4()).is_none());
    }

    #[test]
    fn accounts_for_distinct_ips_independently() {
        let limiter = Arc::new(PerIpLimiter::new(1));
        let ipv4 = v4();
        let ipv6 = IpAddr::V6(Ipv6Addr::LOCALHOST);

        let _v4_permit = limiter.try_acquire(ipv4).expect("IPv4 permit");
        let _v6_permit = limiter.try_acquire(ipv6).expect("IPv6 permit");
        assert!(limiter.try_acquire(ipv4).is_none());
        assert!(limiter.try_acquire(ipv6).is_none());
    }

    #[test]
    fn removes_an_ip_after_its_last_permit_drops() {
        let limiter = Arc::new(PerIpLimiter::new(2));
        let first = limiter.try_acquire(v4()).expect("first permit");
        let second = limiter.try_acquire(v4()).expect("second permit");

        drop(first);
        assert_eq!(limiter.inner.lock().unwrap().get(&v4()), Some(&1));
        drop(second);
        assert!(!limiter.inner.lock().unwrap().contains_key(&v4()));
    }
}
