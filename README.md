# Pyralis Engine
A V4 hook-aware EVM simulation and MEV detection framework in Rust.

I am building this to prototype and validate MEV ideas on Base before anything touches live capital. The engine takes chain data in real time, runs local simulations, scores opportunities, and outputs an execution plan to either dry-run or live execution.

```mermaid
flowchart TD
    A[Base L2 Node] -->|WSS| B[Ingestion<br/>crates/ingestion]
    B -->|tokio::broadcast| C[Simulation<br/>crates/simulation]
    C -->|Vec&lt;Opportunity&gt;| D[Strategy<br/>crates/strategy]
    D -->|ExecutionPlan| E[Executor<br/>crates/executor]
    B -.-> F[Observability]
    C -.-> F
    D -.-> F
    E -.-> F
```

## License
Dual-licensed under MIT and Apache-2.0. See `LICENSE-MIT` and `LICENSE-APACHE`.
