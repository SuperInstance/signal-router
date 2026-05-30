use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, PartialEq)]
pub enum SignalType {
    Tick,
    Murmur,
    Prediction,
    Surprise,
    VibeShift,
    Anomaly,
    GcReport,
    EnergyUpdate,
}

#[derive(Debug, Clone)]
pub struct Signal {
    pub source: String,
    pub target: String,
    pub signal_type: SignalType,
    pub payload: Vec<f64>,
    pub priority: f64,
}

#[derive(Debug, Clone)]
pub enum RouteAlgorithm {
    Direct,
    Buffered { size: usize },
    Correlated { threshold: f64 },
    OnChange,
    Sampled { interval: u64 },
    Adaptive,
}

#[derive(Debug, Clone)]
pub struct Port {
    pub id: String,
    pub filters: Vec<SignalType>,
    pub transform: Option<fn(&mut Signal)>,
}

impl Port {
    pub fn matches(&self, signal: &Signal) -> bool {
        if self.filters.is_empty() {
            return true;
        }
        self.filters.contains(&signal.signal_type)
    }
}

#[derive(Debug, Clone)]
pub struct Route {
    pub from: String,
    pub to: String,
    pub algorithm: RouteAlgorithm,
}

#[derive(Debug, Clone, Default)]
pub struct RouterStats {
    pub signals_routed: u64,
    pub signals_filtered: u64,
    pub signals_dropped: u64,
}

pub struct Router {
    pub routes: Vec<Route>,
    pub ports: Vec<Port>,
    pub stats: RouterStats,
    // Internal state
    pending: HashMap<String, VecDeque<Signal>>,
    buffers: HashMap<String, Vec<Vec<Signal>>>,
    last_payloads: HashMap<String, Vec<f64>>,
    sample_counters: HashMap<String, (u64, u64)>, // (current, interval)
    adaptive_history: HashMap<String, Vec<f64>>,
    signal_timestamps: Vec<u64>,
    tick_counter: u64,
}

impl Router {
    pub fn new() -> Self {
        Router {
            routes: Vec::new(),
            ports: Vec::new(),
            stats: RouterStats::default(),
            pending: HashMap::new(),
            buffers: HashMap::new(),
            last_payloads: HashMap::new(),
            sample_counters: HashMap::new(),
            adaptive_history: HashMap::new(),
            signal_timestamps: Vec::new(),
            tick_counter: 0,
        }
    }

    pub fn add_port(&mut self, port: Port) {
        self.pending.entry(port.id.clone()).or_default();
        self.ports.push(port);
    }

    pub fn add_route(&mut self, route: Route) {
        let key = route.key();
        match &route.algorithm {
            RouteAlgorithm::Buffered { .. } => {
                self.buffers.entry(key).or_default();
            }
            RouteAlgorithm::Sampled { interval } => {
                self.sample_counters.entry(key).or_insert((0, *interval));
            }
            _ => {}
        }
        self.routes.push(route);
    }

    pub fn send(&mut self, signal: Signal) -> Vec<Signal> {
        let mut delivered = Vec::new();
        self.tick_counter += 1;
        self.signal_timestamps.push(self.tick_counter);

        for route in &self.routes.clone() {
            if route.from != signal.source || route.to != signal.target {
                continue;
            }

            let results = self.apply_algorithm(&route, signal.clone());
            for s in results {
                if let Some(port) = self.ports.iter_mut().find(|p| p.id == s.target || p.id == s.source) {
                    if port.matches(&s) {
                        let mut sig = s.clone();
                        if let Some(transform) = port.transform {
                            transform(&mut sig);
                        }
                        self.pending.entry(sig.target.clone()).or_default().push_back(sig.clone());
                        delivered.push(sig);
                        self.stats.signals_routed += 1;
                    } else {
                        self.stats.signals_filtered += 1;
                    }
                } else {
                    // No matching port, deliver anyway
                    self.pending.entry(s.target.clone()).or_default().push_back(s.clone());
                    delivered.push(s);
                    self.stats.signals_routed += 1;
                }
            }
        }

        if delivered.is_empty() && !self.routes.iter().any(|r| r.from == signal.source && r.to == signal.target) {
            self.stats.signals_dropped += 1;
        }

        delivered
    }

    fn apply_algorithm(&mut self, route: &Route, signal: Signal) -> Vec<Signal> {
        let key = route.key();
        match &route.algorithm {
            RouteAlgorithm::Direct => vec![signal],
            RouteAlgorithm::Buffered { size } => {
                let buf = self.buffers.entry(key.clone()).or_default();
                buf.push(vec![signal]);
                if buf.len() >= *size {
                    let drained: Vec<Signal> = buf.drain(..).flatten().collect();
                    drained
                } else {
                    vec![]
                }
            }
            RouteAlgorithm::Correlated { threshold } => {
                if signal.payload.iter().any(|&v| v.abs() >= *threshold) {
                    vec![signal]
                } else {
                    self.stats.signals_filtered += 1;
                    vec![]
                }
            }
            RouteAlgorithm::OnChange => {
                let last = self.last_payloads.get(&key);
                match last {
                    Some(prev) if prev == &signal.payload => {
                        self.stats.signals_filtered += 1;
                        vec![]
                    }
                    _ => {
                        self.last_payloads.insert(key, signal.payload.clone());
                        vec![signal]
                    }
                }
            }
            RouteAlgorithm::Sampled { interval } => {
                let counter = self.sample_counters.entry(key.clone()).or_insert((0, *interval));
                counter.0 += 1;
                if counter.0 >= counter.1 {
                    counter.0 = 0;
                    vec![signal]
                } else {
                    self.stats.signals_filtered += 1;
                    vec![]
                }
            }
            RouteAlgorithm::Adaptive => {
                // Track priority history and select best algorithm
                let hist = self.adaptive_history.entry(key.clone()).or_default();
                hist.push(signal.priority);
                if hist.len() > 100 {
                    hist.remove(0);
                }
                let avg_priority = hist.iter().sum::<f64>() / hist.len() as f64;

                if avg_priority > 0.8 {
                    vec![signal] // Direct for high priority
                } else if avg_priority > 0.5 {
                    let last = self.last_payloads.get(&key);
                    match last {
                        Some(prev) if prev == &signal.payload => {
                            self.stats.signals_filtered += 1;
                            vec![]
                        }
                        _ => {
                            self.last_payloads.insert(key, signal.payload.clone());
                            vec![signal]
                        }
                    }
                } else {
                    // Sample: pass every 3rd
                    let counter = self.sample_counters.entry(key.clone()).or_insert((0, 3));
                    counter.0 += 1;
                    if counter.0 >= 3 {
                        counter.0 = 0;
                        vec![signal]
                    } else {
                        self.stats.signals_filtered += 1;
                        vec![]
                    }
                }
            }
        }
    }

    pub fn receive(&mut self, port_id: &str) -> Vec<Signal> {
        match self.pending.get_mut(port_id) {
            Some(queue) => queue.drain(..).collect(),
            None => vec![],
        }
    }

    pub fn broadcast(&mut self, signal: Signal) -> Vec<Signal> {
        let mut delivered = Vec::new();
        for port in &self.ports.clone() {
            let mut sig = signal.clone();
            sig.target = port.id.clone();
            if port.matches(&sig) {
                if let Some(transform) = port.transform {
                    transform(&mut sig);
                }
                self.pending.entry(port.id.clone()).or_default().push_back(sig.clone());
                delivered.push(sig);
                self.stats.signals_routed += 1;
            } else {
                self.stats.signals_filtered += 1;
            }
        }
        delivered
    }

    pub fn apply_deadband(&self, signal: &Signal, prev: &[f64], threshold: f64) -> bool {
        if signal.payload.len() != prev.len() {
            return true;
        }
        signal.payload.iter().zip(prev.iter()).any(|(&a, &b)| (a - b).abs() > threshold)
    }

    pub fn find_path(&self, from: &str, to: &str) -> Option<Vec<String>> {
        use std::collections::HashSet;
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, Vec<String>)> = VecDeque::new();
        visited.insert(from.to_string());
        queue.push_back((from.to_string(), vec![from.to_string()]));

        while let Some((node, path)) = queue.pop_front() {
            if node == to {
                return Some(path);
            }
            for route in &self.routes {
                if route.from == node && !visited.contains(&route.to) {
                    visited.insert(route.to.clone());
                    let mut new_path = path.clone();
                    new_path.push(route.to.clone());
                    queue.push_back((route.to.clone(), new_path));
                }
            }
        }
        None
    }

    pub fn detect_storm(&mut self, threshold: u64) -> bool {
        // Count signals in last 10 ticks
        let now = self.tick_counter;
        let recent = self.signal_timestamps.iter().filter(|&&t| now.saturating_sub(t) < 10).count() as u64;
        recent > threshold
    }
}

trait RouteKey {
    fn key(&self) -> String;
}

impl RouteKey for Route {
    fn key(&self) -> String {
        format!("{}->{}", self.from, self.to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_signal(source: &str, target: &str, st: SignalType, payload: Vec<f64>, priority: f64) -> Signal {
        Signal { source: source.to_string(), target: target.to_string(), signal_type: st, payload, priority }
    }

    #[test]
    fn test_create_router() {
        let router = Router::new();
        assert!(router.routes.is_empty());
        assert!(router.ports.is_empty());
        assert_eq!(router.stats.signals_routed, 0);
    }

    #[test]
    fn test_add_port_and_route() {
        let mut router = Router::new();
        router.add_port(Port { id: "A".into(), filters: vec![], transform: None });
        router.add_route(Route { from: "A".into(), to: "B".into(), algorithm: RouteAlgorithm::Direct });
        assert_eq!(router.ports.len(), 1);
        assert_eq!(router.routes.len(), 1);
    }

    #[test]
    fn test_direct_routing() {
        let mut router = Router::new();
        router.add_port(Port { id: "B".into(), filters: vec![], transform: None });
        router.add_route(Route { from: "A".into(), to: "B".into(), algorithm: RouteAlgorithm::Direct });
        let delivered = router.send(make_signal("A", "B", SignalType::Tick, vec![1.0], 1.0));
        assert_eq!(delivered.len(), 1);
        let received = router.receive("B");
        assert_eq!(received.len(), 1);
    }

    #[test]
    fn test_buffered_routing() {
        let mut router = Router::new();
        router.add_port(Port { id: "B".into(), filters: vec![], transform: None });
        router.add_route(Route { from: "A".into(), to: "B".into(), algorithm: RouteAlgorithm::Buffered { size: 3 } });

        // First two: buffered, not delivered
        let d1 = router.send(make_signal("A", "B", SignalType::Tick, vec![1.0], 1.0));
        assert!(d1.is_empty());
        let d2 = router.send(make_signal("A", "B", SignalType::Tick, vec![2.0], 1.0));
        assert!(d2.is_empty());

        // Third: flushes buffer
        let d3 = router.send(make_signal("A", "B", SignalType::Tick, vec![3.0], 1.0));
        assert_eq!(d3.len(), 3);
    }

    #[test]
    fn test_correlated_routing() {
        let mut router = Router::new();
        router.add_port(Port { id: "B".into(), filters: vec![], transform: None });
        router.add_route(Route { from: "A".into(), to: "B".into(), algorithm: RouteAlgorithm::Correlated { threshold: 0.5 } });

        let d1 = router.send(make_signal("A", "B", SignalType::Tick, vec![0.1], 1.0));
        assert!(d1.is_empty());

        let d2 = router.send(make_signal("A", "B", SignalType::Tick, vec![0.9], 1.0));
        assert_eq!(d2.len(), 1);
    }

    #[test]
    fn test_on_change_routing() {
        let mut router = Router::new();
        router.add_port(Port { id: "B".into(), filters: vec![], transform: None });
        router.add_route(Route { from: "A".into(), to: "B".into(), algorithm: RouteAlgorithm::OnChange });

        let d1 = router.send(make_signal("A", "B", SignalType::Tick, vec![1.0, 2.0], 1.0));
        assert_eq!(d1.len(), 1);

        let d2 = router.send(make_signal("A", "B", SignalType::Tick, vec![1.0, 2.0], 1.0));
        assert!(d2.is_empty());

        let d3 = router.send(make_signal("A", "B", SignalType::Tick, vec![1.0, 3.0], 1.0));
        assert_eq!(d3.len(), 1);
    }

    #[test]
    fn test_sampled_routing() {
        let mut router = Router::new();
        router.add_port(Port { id: "B".into(), filters: vec![], transform: None });
        router.add_route(Route { from: "A".into(), to: "B".into(), algorithm: RouteAlgorithm::Sampled { interval: 2 } });

        let d1 = router.send(make_signal("A", "B", SignalType::Tick, vec![1.0], 1.0));
        assert!(d1.is_empty());
        let d2 = router.send(make_signal("A", "B", SignalType::Tick, vec![2.0], 1.0));
        assert_eq!(d2.len(), 1);
    }

    #[test]
    fn test_adaptive_routing() {
        let mut router = Router::new();
        router.add_port(Port { id: "B".into(), filters: vec![], transform: None });
        router.add_route(Route { from: "A".into(), to: "B".into(), algorithm: RouteAlgorithm::Adaptive });

        // High priority -> direct pass
        let d = router.send(make_signal("A", "B", SignalType::Tick, vec![1.0], 0.9));
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn test_broadcast() {
        let mut router = Router::new();
        router.add_port(Port { id: "P1".into(), filters: vec![], transform: None });
        router.add_port(Port { id: "P2".into(), filters: vec![], transform: None });
        let delivered = router.broadcast(make_signal("src", "", SignalType::Tick, vec![1.0], 1.0));
        assert_eq!(delivered.len(), 2);
        assert!(delivered.iter().any(|s| s.target == "P1"));
        assert!(delivered.iter().any(|s| s.target == "P2"));
    }

    #[test]
    fn test_find_path() {
        let mut router = Router::new();
        router.add_route(Route { from: "A".into(), to: "B".into(), algorithm: RouteAlgorithm::Direct });
        router.add_route(Route { from: "B".into(), to: "C".into(), algorithm: RouteAlgorithm::Direct });
        let path = router.find_path("A", "C").unwrap();
        assert_eq!(path, vec!["A", "B", "C"]);
    }

    #[test]
    fn test_find_path_disconnected() {
        let mut router = Router::new();
        router.add_route(Route { from: "A".into(), to: "B".into(), algorithm: RouteAlgorithm::Direct });
        assert!(router.find_path("A", "C").is_none());
    }

    #[test]
    fn test_detect_storm() {
        let mut router = Router::new();
        router.add_port(Port { id: "B".into(), filters: vec![], transform: None });
        router.add_route(Route { from: "A".into(), to: "B".into(), algorithm: RouteAlgorithm::Direct });

        // Send a bunch of signals rapidly
        for i in 0..20 {
            router.send(make_signal("A", "B", SignalType::Tick, vec![i as f64], 1.0));
        }
        assert!(router.detect_storm(5));
    }

    #[test]
    fn test_deadband_filters_small() {
        let router = Router::new();
        let signal = make_signal("A", "B", SignalType::Tick, vec![1.0, 2.0], 1.0);
        assert!(!router.apply_deadband(&signal, &[1.01, 2.01], 0.1));
    }

    #[test]
    fn test_deadband_passes_large() {
        let router = Router::new();
        let signal = make_signal("A", "B", SignalType::Tick, vec![1.0, 2.0], 1.0);
        assert!(router.apply_deadband(&signal, &[1.5, 2.0], 0.1));
    }

    #[test]
    fn test_stats_tracking() {
        let mut router = Router::new();
        router.add_port(Port { id: "B".into(), filters: vec![SignalType::Tick], transform: None });
        router.add_route(Route { from: "A".into(), to: "B".into(), algorithm: RouteAlgorithm::Direct });

        // Routed
        router.send(make_signal("A", "B", SignalType::Tick, vec![1.0], 1.0));
        // Filtered (wrong type)
        router.send(make_signal("A", "B", SignalType::Murmur, vec![1.0], 1.0));
        // Dropped (no matching route)
        router.send(make_signal("X", "Y", SignalType::Tick, vec![1.0], 1.0));

        assert_eq!(router.stats.signals_routed, 1);
        assert_eq!(router.stats.signals_filtered, 1);
        assert_eq!(router.stats.signals_dropped, 1);
    }

    #[test]
    fn test_port_matches() {
        let port = Port { id: "A".into(), filters: vec![SignalType::Tick, SignalType::Anomaly], transform: None };
        assert!(port.matches(&make_signal("X", "A", SignalType::Tick, vec![], 1.0)));
        assert!(!port.matches(&make_signal("X", "A", SignalType::Murmur, vec![], 1.0)));
    }

    #[test]
    fn test_port_transform() {
        let double: fn(&mut Signal) = |s| s.payload.iter_mut().for_each(|v| *v *= 2.0);
        let mut router = Router::new();
        router.add_port(Port { id: "B".into(), filters: vec![], transform: Some(double) });
        router.add_route(Route { from: "A".into(), to: "B".into(), algorithm: RouteAlgorithm::Direct });
        let delivered = router.send(make_signal("A", "B", SignalType::Tick, vec![3.0, 4.0], 1.0));
        assert_eq!(delivered[0].payload, vec![6.0, 8.0]);
    }
}
