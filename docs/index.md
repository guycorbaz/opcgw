---
layout: default
title: opcgw - ChirpStack to OPC UA Gateway
---

<section class="hero">
    <div class="container">
        <div class="row align-items-center">
            <div class="col-lg-6 hero-content">
                <h1>Bridge IoT to Industrial Systems</h1>
                <p class="hero-subtitle">Connect ChirpStack LoRaWAN devices to OPC UA SCADA systems with production-ready reliability</p>
                <div class="d-flex gap-2 flex-wrap">
                    <a href="{{ site.baseurl }}/quickstart/" class="btn btn-primary-green">Get Started</a>
                    <a href="https://github.com/guycorbaz/opcgw" target="_blank" class="btn btn-secondary-blue">View on GitHub</a>
                </div>
                <div style="margin-top: 3rem;">
                    <span class="badge-custom">✨ v2.0.0 - Production Ready</span>
                    <span class="badge-custom">🦀 Written in Rust</span>
                    <span class="badge-custom">📡 OPC UA 1.04</span>
                </div>
            </div>
            <div class="col-lg-6 text-center">
                <div style="font-size: 4rem; color: rgba(52, 211, 153, 0.3); margin-bottom: 2rem;">
                    <i class="fas fa-project-diagram"></i>
                </div>
            </div>
        </div>
    </div>
</section>

<section class="stats-section">
    <div class="container">
        <div class="row">
            <div class="col-md-3">
                <div class="stat-card">
                    <div class="stat-number">2.0</div>
                    <div class="stat-label">Version</div>
                </div>
            </div>
            <div class="col-md-3">
                <div class="stat-card">
                    <div class="stat-number">5</div>
                    <div class="stat-label">Completed Stories</div>
                </div>
            </div>
            <div class="col-md-3">
                <div class="stat-card">
                    <div class="stat-number">100%</div>
                    <div class="stat-label">Test Coverage</div>
                </div>
            </div>
            <div class="col-md-3">
                <div class="stat-card">
                    <div class="stat-number">MIT/Apache</div>
                    <div class="stat-label">Open License</div>
                </div>
            </div>
        </div>
    </div>
</section>

<section class="content-section">
    <div class="container">
        <h2 class="text-center mb-5">Why opcgw?</h2>
        <div class="row">
            <div class="col-lg-6 mb-4">
                <div class="feature-card">
                    <div class="feature-icon"><i class="fas fa-network-wired"></i></div>
                    <h3 class="feature-title">Real-Time Data Bridge</h3>
                    <p class="feature-text">Continuously polls ChirpStack via gRPC and exposes metrics as OPC UA variables for your SCADA systems</p>
                </div>
            </div>
            <div class="col-lg-6 mb-4">
                <div class="feature-card">
                    <div class="feature-icon"><i class="fas fa-shield-alt"></i></div>
                    <h3 class="feature-title">Production Ready</h3>
                    <p class="feature-text">Built in Rust with comprehensive error handling, configuration validation, and graceful shutdown</p>
                </div>
            </div>
            <div class="col-lg-6 mb-4">
                <div class="feature-card">
                    <div class="feature-icon"><i class="fas fa-cog"></i></div>
                    <h3 class="feature-title">Easy Configuration</h3>
                    <p class="feature-text">Simple TOML configuration with environment variable overrides and startup validation</p>
                </div>
            </div>
            <div class="col-lg-6 mb-4">
                <div class="feature-card">
                    <div class="feature-icon"><i class="fas fa-docker"></i></div>
                    <h3 class="feature-title">Container Native</h3>
                    <p class="feature-text">Official Docker image, Docker Compose setup, and Kubernetes-ready with health checks</p>
                </div>
            </div>
            <div class="col-lg-6 mb-4">
                <div class="feature-card">
                    <div class="feature-icon"><i class="fas fa-chart-line"></i></div>
                    <h3 class="feature-title">Scalable Architecture</h3>
                    <p class="feature-text">Support for hundreds of devices with configurable polling intervals and automatic retry logic</p>
                </div>
            </div>
            <div class="col-lg-6 mb-4">
                <div class="feature-card">
                    <div class="feature-icon"><i class="fas fa-eye"></i></div>
                    <h3 class="feature-title">Observable</h3>
                    <p class="feature-text">Structured logging with per-module log files for deep visibility into gateway operation</p>
                </div>
            </div>
        </div>
    </div>
</section>

<section style="background: var(--light-bg); padding: 4rem 0;">
    <div class="container">
        <h2 class="text-center mb-5">Quick Start</h2>
        <div class="row">
            <div class="col-lg-8 offset-lg-2">
                <div class="code-block">
<pre style="margin: 0; color: inherit;">git clone https://github.com/guycorbaz/opcgw.git
cd opcgw
cp config/config.example.toml config/config.toml
# Edit config/config.toml with your ChirpStack details
cargo run --release -- -c config/config.toml</pre>
                </div>
                <p class="text-center" style="color: #64748b; margin-top: 2rem;">Or use Docker:</p>
                <div class="code-block">
<pre style="margin: 0; color: inherit;">docker-compose up</pre>
                </div>
                <div class="text-center mt-4">
                    <a href="{{ site.baseurl }}/quickstart/" class="btn btn-primary-green">Read Full Guide →</a>
                </div>
            </div>
        </div>
    </div>
</section>

<section class="content-section">
    <div class="container">
        <h2 class="text-center mb-5">Use Cases</h2>
        <div class="row">
            <div class="col-md-6 mb-4">
                <div style="background: white; border-radius: 12px; padding: 2rem; border-left: 4px solid var(--primary-green);">
                    <h4 style="color: var(--dark-bg); font-weight: 700;">🌱 Smart Agriculture</h4>
                    <p style="color: #64748b; margin: 1rem 0;">Monitor soil conditions, optimize irrigation, prevent crop loss</p>
                    <a href="{{ site.baseurl }}/usecases/#smart-agriculture-precision-field-monitoring" style="color: var(--primary-green); font-weight: 600;">Learn more →</a>
                </div>
            </div>
            <div class="col-md-6 mb-4">
                <div style="background: white; border-radius: 12px; padding: 2rem; border-left: 4px solid var(--secondary-blue);">
                    <h4 style="color: var(--dark-bg); font-weight: 700;">🏭 Industrial IoT</h4>
                    <p style="color: #64748b; margin: 1rem 0;">Real-time equipment monitoring, predictive maintenance</p>
                    <a href="{{ site.baseurl }}/usecases/#smart-factory-real-time-equipment-monitoring" style="color: var(--secondary-blue); font-weight: 600;">Learn more →</a>
                </div>
            </div>
            <div class="col-md-6 mb-4">
                <div style="background: white; border-radius: 12px; padding: 2rem; border-left: 4px solid var(--primary-green);">
                    <h4 style="color: var(--dark-bg); font-weight: 700;">🌍 Environmental Monitoring</h4>
                    <p style="color: #64748b; margin: 1rem 0;">Air quality networks, public health alerts, compliance</p>
                    <a href="{{ site.baseurl }}/usecases/#environmental-monitoring-urban-air-quality-network" style="color: var(--primary-green); font-weight: 600;">Learn more →</a>
                </div>
            </div>
            <div class="col-md-6 mb-4">
                <div style="background: white; border-radius: 12px; padding: 2rem; border-left: 4px solid var(--secondary-blue);">
                    <h4 style="color: var(--dark-bg); font-weight: 700;">🏢 Building Automation</h4>
                    <p style="color: #64748b; margin: 1rem 0;">HVAC optimization, energy savings, occupancy-based control</p>
                    <a href="{{ site.baseurl }}/usecases/#building-automation-energy-management-at-scale" style="color: var(--secondary-blue); font-weight: 600;">Learn more →</a>
                </div>
            </div>
        </div>
        <div class="text-center mt-4">
            <a href="{{ site.baseurl }}/usecases/" class="btn btn-primary-green">See All Use Cases →</a>
        </div>
    </div>
</section>

<section style="background: linear-gradient(135deg, var(--primary-green), var(--secondary-blue)); color: white; padding: 4rem 0; text-align: center;">
    <div class="container">
        <h2 style="font-size: 2rem; margin-bottom: 1rem;">Ready to bridge your systems?</h2>
        <p style="font-size: 1.1rem; margin-bottom: 2rem; opacity: 0.9;">Start with our comprehensive documentation and community support</p>
        <div class="d-flex gap-2 justify-content-center flex-wrap">
            <a href="{{ site.baseurl }}/quickstart/" class="btn btn-light">Quick Start Guide</a>
            <a href="https://github.com/guycorbaz/opcgw" target="_blank" class="btn btn-outline-light">GitHub Repository</a>
        </div>
    </div>
</section>

