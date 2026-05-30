//! signal-router: Signal routing with Signal, Port, Route, Router.
//! Six algorithms, deadband, filter/transform.

use std::collections::HashMap;

/// A scalar signal value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Signal {
    pub value: f32,
    pub timestamp_ms: u64,
}

impl Signal {
    pub fn new(value: f32, timestamp_ms: u64) -> Self {
        Self { value, timestamp_ms }
    }

    pub fn zero() -> Self {
        Self {
            value: 0.0,
            timestamp_ms: 0,
        }
    }

    pub fn scale(&self, factor: f32) -> Self {
        Self {
            value: self.value * factor,
            timestamp_ms: self.timestamp_ms,
        }
    }

    pub fn offset(&self, delta: f32) -> Self {
        Self {
            value: self.value + delta,
            timestamp_ms: self.timestamp_ms,
        }
    }

    pub fn clamp(&self, min: f32, max: f32) -> Self {
        Self {
            value: self.value.clamp(min, max),
            timestamp_ms: self.timestamp_ms,
        }
    }
}

/// A port on a router (input or output).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Port {
    pub name: String,
    pub is_input: bool,
}

impl Port {
    pub fn input(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            is_input: true,
        }
    }

    pub fn output(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            is_input: false,
        }
    }
}

/// A route connecting an input port to an output port.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub input: String,
    pub output: String,
    pub algorithm: RoutingAlgorithm,
    pub deadband: f32,
    pub transform: Option<TransformOp>,
}

impl Route {
    pub fn new(input: impl Into<String>, output: impl Into<String>, algorithm: RoutingAlgorithm) -> Self {
        Self {
            input: input.into(),
            output: output.into(),
            algorithm,
            deadband: 0.0,
            transform: None,
        }
    }

    pub fn with_deadband(mut self, threshold: f32) -> Self {
        self.deadband = threshold;
        self
    }

    pub fn with_transform(mut self, op: TransformOp) -> Self {
        self.transform = Some(op);
        self
    }
}

/// Routing algorithms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoutingAlgorithm {
    Passthrough,
    Average,
    Maximum,
    Minimum,
    LastWins,
    Sum,
}

/// A transform operation applied to a signal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TransformOp {
    Scale(f32),
    Offset(f32),
    Clamp(f32, f32),
    Invert,
    Deadband(f32),
}

impl TransformOp {
    pub fn apply(&self, signal: Signal) -> Signal {
        match self {
            TransformOp::Scale(s) => signal.scale(*s),
            TransformOp::Offset(o) => signal.offset(*o),
            TransformOp::Clamp(min, max) => signal.clamp(*min, *max),
            TransformOp::Invert => Signal::new(-signal.value, signal.timestamp_ms),
            TransformOp::Deadband(th) => {
                if signal.value.abs() < *th {
                    Signal::new(0.0, signal.timestamp_ms)
                } else {
                    signal
                }
            }
        }
    }
}

/// The router that manages ports, routes, and signal distribution.
#[derive(Debug, Clone, Default)]
pub struct Router {
    pub ports: Vec<Port>,
    pub routes: Vec<Route>,
    pub input_values: HashMap<String, Signal>,
    pub output_values: HashMap<String, Signal>,
    pub last_output_values: HashMap<String, Signal>,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_port(&mut self, port: Port) {
        self.ports.push(port);
    }

    pub fn add_route(&mut self, route: Route) {
        self.routes.push(route);
    }

    pub fn set_input(&mut self, name: impl Into<String>, signal: Signal) {
        self.input_values.insert(name.into(), signal);
    }

    pub fn get_output(&self, name: &str) -> Option<Signal> {
        self.output_values.get(name).copied()
    }

    pub fn get_input(&self, name: &str) -> Option<Signal> {
        self.input_values.get(name).copied()
    }

    /// Process all routes and compute outputs.
    pub fn process(&mut self) {
        self.last_output_values = self.output_values.clone();
        self.output_values.clear();

        let mut bucketed: HashMap<String, Vec<(Signal, &Route)>> = HashMap::new();
        for route in &self.routes {
            if let Some(sig) = self.input_values.get(&route.input).copied() {
                bucketed
                    .entry(route.output.clone())
                    .or_default()
                    .push((sig, route));
            }
        }

        for (output_name, inputs) in bucketed {
            let mut values: Vec<f32> = Vec::new();
            let mut latest_ts = 0u64;
            for (sig, route) in inputs {
                let mut v = sig;
                if let Some(t) = &route.transform {
                    v = t.apply(v);
                }
                if route.deadband > 0.0 {
                    if v.value.abs() < route.deadband {
                        v.value = 0.0;
                    }
                }
                values.push(v.value);
                if v.timestamp_ms > latest_ts {
                    latest_ts = v.timestamp_ms;
                }
            }

            if values.is_empty() {
                continue;
            }

            let result = match self.routes.iter().find(|r| r.output == output_name).map(|r| r.algorithm) {
                Some(RoutingAlgorithm::Average) => values.iter().sum::<f32>() / values.len() as f32,
                Some(RoutingAlgorithm::Maximum) => values.iter().copied().fold(f32::NEG_INFINITY, f32::max),
                Some(RoutingAlgorithm::Minimum) => values.iter().copied().fold(f32::INFINITY, f32::min),
                Some(RoutingAlgorithm::LastWins) => *values.last().unwrap(),
                Some(RoutingAlgorithm::Sum) => values.iter().sum::<f32>(),
                _ => values[0],
            };

            self.output_values.insert(output_name, Signal::new(result, latest_ts));
        }
    }

    pub fn port_count(&self) -> usize {
        self.ports.len()
    }

    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// Apply a filter to an input (simple first-order low-pass).
    pub fn apply_lowpass(&mut self, input_name: &str, alpha: f32) -> Option<Signal> {
        let current = self.input_values.get(input_name).copied()?;
        let prev = self.last_output_values.get(input_name).copied().unwrap_or(current);
        let filtered = prev.value + alpha * (current.value - prev.value);
        let sig = Signal::new(filtered, current.timestamp_ms);
        self.input_values.insert(input_name.to_string(), sig);
        self.last_output_values.insert(input_name.to_string(), sig);
        Some(sig)
    }

    pub fn clear(&mut self) {
        self.input_values.clear();
        self.output_values.clear();
        self.last_output_values.clear();
    }
}

/// A patch bay that wires routers together.
#[derive(Debug, Clone, Default)]
pub struct PatchBay {
    pub routers: HashMap<String, Router>,
}

impl PatchBay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_router(&mut self, name: impl Into<String>, router: Router) {
        self.routers.insert(name.into(), router);
    }

    pub fn get_router(&mut self, name: &str) -> Option<&mut Router> {
        self.routers.get_mut(name)
    }

    pub fn router_count(&self) -> usize {
        self.routers.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signal_new() {
        let s = Signal::new(3.5, 100);
        assert_eq!(s.value, 3.5);
        assert_eq!(s.timestamp_ms, 100);
    }

    #[test]
    fn test_signal_zero() {
        let s = Signal::zero();
        assert_eq!(s.value, 0.0);
    }

    #[test]
    fn test_signal_scale() {
        let s = Signal::new(2.0, 0);
        let t = s.scale(3.0);
        assert_eq!(t.value, 6.0);
    }

    #[test]
    fn test_signal_offset() {
        let s = Signal::new(2.0, 0);
        let t = s.offset(-1.0);
        assert_eq!(t.value, 1.0);
    }

    #[test]
    fn test_signal_clamp() {
        let s = Signal::new(10.0, 0);
        let t = s.clamp(0.0, 5.0);
        assert_eq!(t.value, 5.0);
    }

    #[test]
    fn test_port_input() {
        let p = Port::input("in1");
        assert!(p.is_input);
        assert_eq!(p.name, "in1");
    }

    #[test]
    fn test_port_output() {
        let p = Port::output("out1");
        assert!(!p.is_input);
        assert_eq!(p.name, "out1");
    }

    #[test]
    fn test_route_creation() {
        let r = Route::new("in1", "out1", RoutingAlgorithm::Passthrough);
        assert_eq!(r.input, "in1");
        assert_eq!(r.output, "out1");
        assert_eq!(r.deadband, 0.0);
    }

    #[test]
    fn test_route_with_deadband() {
        let r = Route::new("in1", "out1", RoutingAlgorithm::Passthrough).with_deadband(0.1);
        assert_eq!(r.deadband, 0.1);
    }

    #[test]
    fn test_route_with_transform() {
        let r = Route::new("in1", "out1", RoutingAlgorithm::Passthrough).with_transform(TransformOp::Scale(2.0));
        assert!(matches!(r.transform, Some(TransformOp::Scale(2.0))));
    }

    #[test]
    fn test_transform_scale() {
        let s = Signal::new(3.0, 0);
        let t = TransformOp::Scale(2.0).apply(s);
        assert_eq!(t.value, 6.0);
    }

    #[test]
    fn test_transform_offset() {
        let s = Signal::new(3.0, 0);
        let t = TransformOp::Offset(-1.0).apply(s);
        assert_eq!(t.value, 2.0);
    }

    #[test]
    fn test_transform_clamp() {
        let s = Signal::new(10.0, 0);
        let t = TransformOp::Clamp(0.0, 5.0).apply(s);
        assert_eq!(t.value, 5.0);
    }

    #[test]
    fn test_transform_invert() {
        let s = Signal::new(4.0, 0);
        let t = TransformOp::Invert.apply(s);
        assert_eq!(t.value, -4.0);
    }

    #[test]
    fn test_transform_deadband() {
        let s = Signal::new(0.05, 0);
        let t = TransformOp::Deadband(0.1).apply(s);
        assert_eq!(t.value, 0.0);
        let s2 = Signal::new(0.2, 0);
        let t2 = TransformOp::Deadband(0.1).apply(s2);
        assert_eq!(t2.value, 0.2);
    }

    #[test]
    fn test_router_passthrough() {
        let mut router = Router::new();
        router.add_port(Port::input("in1"));
        router.add_port(Port::output("out1"));
        router.add_route(Route::new("in1", "out1", RoutingAlgorithm::Passthrough));
        router.set_input("in1", Signal::new(5.0, 0));
        router.process();
        assert_eq!(router.get_output("out1").unwrap().value, 5.0);
    }

    #[test]
    fn test_router_average() {
        let mut router = Router::new();
        router.add_route(Route::new("in1", "out1", RoutingAlgorithm::Average));
        router.add_route(Route::new("in2", "out1", RoutingAlgorithm::Average));
        router.set_input("in1", Signal::new(10.0, 0));
        router.set_input("in2", Signal::new(20.0, 0));
        router.process();
        assert_eq!(router.get_output("out1").unwrap().value, 15.0);
    }

    #[test]
    fn test_router_maximum() {
        let mut router = Router::new();
        router.add_route(Route::new("in1", "out1", RoutingAlgorithm::Maximum));
        router.add_route(Route::new("in2", "out1", RoutingAlgorithm::Maximum));
        router.set_input("in1", Signal::new(5.0, 0));
        router.set_input("in2", Signal::new(8.0, 0));
        router.process();
        assert_eq!(router.get_output("out1").unwrap().value, 8.0);
    }

    #[test]
    fn test_router_minimum() {
        let mut router = Router::new();
        router.add_route(Route::new("in1", "out1", RoutingAlgorithm::Minimum));
        router.add_route(Route::new("in2", "out1", RoutingAlgorithm::Minimum));
        router.set_input("in1", Signal::new(5.0, 0));
        router.set_input("in2", Signal::new(8.0, 0));
        router.process();
        assert_eq!(router.get_output("out1").unwrap().value, 5.0);
    }

    #[test]
    fn test_router_last_wins() {
        let mut router = Router::new();
        router.add_route(Route::new("in1", "out1", RoutingAlgorithm::LastWins));
        router.add_route(Route::new("in2", "out1", RoutingAlgorithm::LastWins));
        router.set_input("in1", Signal::new(5.0, 0));
        router.set_input("in2", Signal::new(9.0, 0));
        router.process();
        assert_eq!(router.get_output("out1").unwrap().value, 9.0);
    }

    #[test]
    fn test_router_sum() {
        let mut router = Router::new();
        router.add_route(Route::new("in1", "out1", RoutingAlgorithm::Sum));
        router.add_route(Route::new("in2", "out1", RoutingAlgorithm::Sum));
        router.set_input("in1", Signal::new(2.0, 0));
        router.set_input("in2", Signal::new(3.0, 0));
        router.process();
        assert_eq!(router.get_output("out1").unwrap().value, 5.0);
    }

    #[test]
    fn test_router_deadband() {
        let mut router = Router::new();
        router.add_route(Route::new("in1", "out1", RoutingAlgorithm::Passthrough).with_deadband(0.5));
        router.set_input("in1", Signal::new(0.2, 0));
        router.process();
        assert_eq!(router.get_output("out1").unwrap().value, 0.0);
    }

    #[test]
    fn test_router_transform_in_route() {
        let mut router = Router::new();
        router.add_route(
            Route::new("in1", "out1", RoutingAlgorithm::Passthrough).with_transform(TransformOp::Scale(3.0)),
        );
        router.set_input("in1", Signal::new(2.0, 0));
        router.process();
        assert_eq!(router.get_output("out1").unwrap().value, 6.0);
    }

    #[test]
    fn test_router_lowpass() {
        let mut router = Router::new();
        router.set_input("in1", Signal::new(10.0, 0));
        router.apply_lowpass("in1", 0.5);
        let v = router.get_input("in1").unwrap();
        assert_eq!(v.value, 10.0);
        router.set_input("in1", Signal::new(20.0, 1));
        router.apply_lowpass("in1", 0.5);
        let v2 = router.get_input("in1").unwrap();
        assert_eq!(v2.value, 15.0);
    }

    #[test]
    fn test_patch_bay() {
        let mut bay = PatchBay::new();
        bay.add_router("main", Router::new());
        assert_eq!(bay.router_count(), 1);
        assert!(bay.get_router("main").is_some());
    }

    #[test]
    fn test_router_clear() {
        let mut router = Router::new();
        router.set_input("in1", Signal::new(1.0, 0));
        router.clear();
        assert!(router.get_input("in1").is_none());
    }
}
