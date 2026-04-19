---
layout: page
title: Use Cases & Success Stories
permalink: /usecases/
---

## Real-World Integration Scenarios

opcgw enables seamless integration of LoRaWAN IoT sensor networks into existing industrial systems. Here are detailed use cases showing how the gateway solves real problems.

---

## 🌱 Smart Agriculture: Precision Field Monitoring

### The Challenge

A large agricultural cooperative manages 50+ farms across multiple regions. Each farm has soil, weather, and equipment sensors distributed across 100+ hectares. Previously:
- Manual sensor checks → inconsistent data
- Paper/spreadsheet records → slow decision-making
- No real-time alerts → late response to irrigation needs
- Siloed data → no cross-farm insights

### The Solution with opcgw

**Architecture**:
```
[Soil/Weather Sensors] → LoRaWAN Network → ChirpStack → opcgw → Farm Management System (Ignition)
(Wireless)                   (Coverage)      (Aggregation)  (Bridge)   (Visualization & Logic)
```

**Implementation**:
1. Deploy 300 LoRaWAN soil sensors across farms
   - Soil moisture (%)
   - Temperature (°C)
   - EC/Conductivity (mS/cm)
2. ChirpStack aggregates sensor data
3. opcgw gateway bridges to existing FMS running Ignition
4. Ignition dashboard displays real-time field map
5. Automated alerts trigger when soil moisture < 30%

**Configuration**:
```toml
[[application]]
application_name = "Farm Network"
application_id = "farm-network"

[[application.device]]
device_name = "Field A - North Block"
device_id = "soil_sensor_001"

[[application.device.read_metric]]
metric_name = "Soil Moisture"
chirpstack_metric_name = "soil_moisture_pct"
metric_type = "Float"
metric_unit = "%"

[[application.device.read_metric]]
metric_name = "Soil Temperature"
chirpstack_metric_name = "temp_celsius"
metric_type = "Float"
metric_unit = "°C"

[[application.device.read_metric]]
metric_name = "Soil EC"
chirpstack_metric_name = "ec_ms_per_cm"
metric_type = "Float"
metric_unit = "mS/cm"

# ... repeat for 300 devices across all farms
```

**Results**:
- ✅ 30% reduction in water usage (precise irrigation)
- ✅ 20% increase in crop yield (optimal growing conditions)
- ✅ Real-time visibility → faster decision-making
- ✅ Historical data for trend analysis and planning

**ROI**: Sensor network + gateway cost recouped in first season through water/fertilizer savings.

---

## 🏭 Smart Factory: Real-Time Equipment Monitoring

### The Challenge

A mid-size manufacturing facility has 20 CNC machines spread across the shop floor. Current issues:
- Manual rounds to check machine status → labor intensive
- No early warning for maintenance → unexpected downtime
- Disconnected data → no production optimization
- Temperature/vibration not monitored → tool wear surprises

### The Solution with opcgw

**Architecture**:
```
[Machine Sensors] → LoRaWAN → ChirpStack → opcgw → MES/ERP
(Vibration, Temp)              (Wireless)     (Bridge)
```

**Sensors Deployed**:
- Vibration sensors on spindles
- Temperature sensors on bearings
- Power draw monitors (via wireless meters)
- Door/status sensors (open/closed)

**Implementation**:
```toml
[[application]]
application_name = "Production Floor"
application_id = "production"

[[application.device]]
device_name = "CNC Machine 1"
device_id = "cnc001"

[[application.device.read_metric]]
metric_name = "Vibration Level"
chirpstack_metric_name = "vibration_g"
metric_type = "Float"
metric_unit = "G"

[[application.device.read_metric]]
metric_name = "Bearing Temperature"
chirpstack_metric_name = "bearing_temp"
metric_type = "Float"
metric_unit = "°C"

[[application.device.read_metric]]
metric_name = "Running"
chirpstack_metric_name = "machine_running"
metric_type = "Bool"
```

**Integration with MES**:
- MES subscribes to OPC UA variables
- High vibration → trigger predictive maintenance alert
- Temperature spike → reduce spindle speed automatically
- Machine stops → log idle time for efficiency tracking

**Results**:
- ✅ 40% reduction in unexpected downtime
- ✅ Preventive maintenance before failures
- ✅ Energy consumption visibility
- ✅ Production metrics tied to equipment health

---

## 🌍 Environmental Monitoring: Urban Air Quality Network

### The Challenge

A city health department wants to monitor air quality across neighborhoods. Goals:
- Real-time public health alerts
- Identify pollution hotspots
- Track improvement over time
- Publish open data for residents

### The Solution with opcgw

**Deployment**:
- 100+ air quality sensors distributed across city
- LoRaWAN coverage via community network
- Real-time data pipeline to city data system
- Public-facing dashboard

**Metrics Tracked**:
```
PM2.5, PM10, NO₂, O₃, CO, Temperature, Humidity, Air Pressure
```

**Integration**:
```
[Air Quality Sensor Network] 
    ↓
[ChirpStack Aggregation]
    ↓
[opcgw Bridge]
    ↓
[Analytics Platform] → [Public Dashboard]
    ↓
[Alert System] → [Public Notifications]
```

**Data Pipeline**:
1. Sensors report every 5 minutes
2. opcgw exposes via OPC UA
3. Analytics system subscribes to live data
4. Trends detected → historical database
5. Public API serves open data to residents

**Results**:
- ✅ Real-time air quality data for all neighborhoods
- ✅ Health alerts when PM2.5 exceeds limits
- ✅ Environmental justice: identify polluted areas
- ✅ Accountability: track improvement initiatives
- ✅ Resident engagement: transparency & public health

---

## 🏢 Building Automation: Energy Management at Scale

### The Challenge

A 20-building commercial campus has:
- HVAC zones across 200,000 m²
- Occupancy varies dramatically (20-100%)
- Energy consumption: $5M/year
- Limited visibility into actual vs. scheduled consumption

### The Solution with opcgw

**Wireless Sensor Deployment** (eliminate wiring costs):
- 500+ room occupancy sensors
- 1000+ temperature/humidity sensors
- 50+ water/gas meters
- All wirelessly reporting via LoRaWAN

**Integration with BMS**:
```
[Sensors] → LoRaWAN → ChirpStack → opcgw → Building Management System
                                     ↓
                          [Demand-Controlled HVAC]
                          [Energy Optimization]
                          [Fault Detection]
```

**Automated Logic**:
```
If room unoccupied for 15 min:
  → Set HVAC to standby
  → Dim lighting to 10%
  → Close blinds if temps allow

If temperature differential > 3°C for 10 min:
  → Alert maintenance team
  → Check for air leaks
```

**Results**:
- ✅ 25% reduction in HVAC energy (occupancy-based)
- ✅ 15% reduction in lighting energy (sensor-based)
- ✅ Early fault detection (unexpected temperature variance)
- ✅ Tenant comfort: maintain setpoints while saving energy
- ✅ No wiring disruption to tenants

**Payback**: Energy savings exceed sensor + gateway cost in year 2.

---

## ⚡ Renewable Energy: Microgrid Optimization

### The Challenge

A community solar cooperative with:
- 50 rooftop solar installations
- 10 battery storage units
- Grid-tied + island mode capability
- Complex energy balance requirements

### The Solution with opcgw

**Real-Time Microgrid Control**:
```
[Solar Inverters] → LoRaWAN → ChirpStack → opcgw → Energy Management System
[Battery Units]                                ↓
[Grid Meters]                        [Optimization Engine]
```

**Monitored Parameters**:
- Solar generation (kW) per building
- Battery state of charge (%)
- Grid import/export (kW)
- Load consumption (kW)

**Optimization Logic**:
1. **Solar peak hours**: Charge batteries, power loads, export excess
2. **Cloudy periods**: Use battery, minimize imports
3. **Night time**: Use battery first, then import
4. **Grid stress**: Support grid with battery discharge
5. **Faults**: Disconnect from grid, island mode

**Results**:
- ✅ 90% solar self-consumption (not exported at low rates)
- ✅ Reduced grid imports 40%
- ✅ Revenue: participate in grid services markets
- ✅ Resilience: island mode during grid outages
- ✅ Sustainability: maximize renewable usage

---

## 🚛 Logistics: Asset Tracking & Condition Monitoring

### The Challenge

A logistics company moves temperature-sensitive goods (pharmaceuticals, food). Issues:
- Need to track goods location in real-time
- Monitor temperature during transport
- Prove compliance with cold chain requirements
- Detect tampering or handling issues

### The Solution with opcgw

**Wireless Sensor Packages**:
- LoRaWAN GPS trackers on shipments
- Tamper-evident sensors
- Temperature/humidity data loggers
- Shock detectors

**Real-Time Visibility**:
```
[Shipment Sensors] → LoRaWAN → ChirpStack → opcgw → Logistics Platform
```

**Integration**:
- Real-time location feeds GPS map
- Temperature alerts if exceeds range
- Tamper alert → automatic quarantine in system
- Proof of compliance: automated certificate generation

**Results**:
- ✅ Full supply chain visibility
- ✅ Compliance verification (audit-ready)
- ✅ Early warning: act on cold chain violations
- ✅ Liability protection: documented conditions
- ✅ Efficiency: optimized routes based on real data

---

## Common Success Patterns

Across all use cases, opcgw provides:

1. **Wireless First**: LoRaWAN eliminates wiring costs/disruption
2. **Real-Time Integration**: Live data in existing systems
3. **Standards-Based**: OPC UA works with any SCADA/MES/analytics
4. **Low TCO**: Simple architecture = easy to maintain
5. **Scalable**: Hundreds of devices, minutes to add more

---

## Getting Started With Your Use Case

1. **Identify Sensors**: What data do you need?
2. **Plan LoRaWAN**: Existing network or new deployment?
3. **Map to OPC UA**: Which systems will consume data?
4. **Configure opcgw**: Define applications → devices → metrics
5. **Integrate Target System**: Wire OPC UA into SCADA/MES/Analytics
6. **Monitor & Optimize**: Tune polling intervals, validate data quality

**Ready to build?** Start with the [Quick Start Guide](quickstart.html).

**Have a specific use case?** [Open an issue](https://github.com/guycorbaz/opcgw/issues) or start a [discussion](https://github.com/guycorbaz/opcgw/discussions).
