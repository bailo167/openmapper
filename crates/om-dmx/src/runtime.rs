// SPDX-License-Identifier: Apache-2.0
//! Follows the project's DMX setup: samples each canvas frame for every
//! enabled fixture and hands the universes to the sender.

use om_project::Project;
use om_project::dmx::Dmx;

use crate::mapping::{FixturePlan, FrameView, Sampler, Universes, render_fixtures};
use crate::net::{DmxSender, Ports, SenderStats};
use crate::sacn;

/// DMX output for one project.
#[derive(Debug, Default)]
pub struct DmxRuntime {
    ports: Ports,
    synced: Option<Dmx>,
    plans: Vec<FixturePlan>,
    sampler: Sampler,
    sender: Option<DmxSender>,
    last: Universes,
}

impl DmxRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sends to `ports` instead of the standard ones (tests).
    #[must_use]
    pub fn with_ports(ports: Ports) -> Self {
        Self {
            ports,
            ..Self::default()
        }
    }

    /// Rebuilds fixture plans and sender settings when the project's DMX
    /// setup changed. Cheap when nothing changed.
    pub fn sync(&mut self, project: &Project) {
        if self.synced.as_ref() == Some(&project.dmx) {
            return;
        }
        let dmx = project.dmx.clone();
        self.plans = dmx
            .fixtures
            .iter()
            .filter(|f| f.enabled)
            .filter(|f| dmx.nodes.iter().any(|n| n.id == f.node && n.enabled))
            .map(FixturePlan::new)
            .collect();
        if self.plans.is_empty() {
            // Stopping the sender terminates sACN streams cleanly.
            self.sender = None;
            self.last.clear();
        } else {
            let sender = self.sender.get_or_insert_with(|| {
                DmxSender::spawn_with(
                    sacn::Source {
                        cid: project.project_id.as_u128().to_be_bytes(),
                        name: format!("OpenMapper – {}", project.name),
                    },
                    self.ports,
                )
            });
            sender.configure(&dmx.nodes, dmx.rate);
        }
        self.synced = Some(dmx);
    }

    /// True when fixtures want canvas frames (callers skip readback
    /// otherwise).
    #[must_use]
    pub fn is_active(&self) -> bool {
        !self.plans.is_empty()
    }

    /// Samples `frame` for every fixture and sends the result.
    pub fn submit(&mut self, frame: &FrameView<'_>) {
        if self.plans.is_empty() {
            return;
        }
        let universes = render_fixtures(&self.plans, &self.sampler, frame);
        if let Some(s) = &self.sender {
            s.submit(universes.clone());
        }
        self.last = universes;
    }

    /// The universes sent most recently (for the UI's channel view).
    #[must_use]
    pub fn last(&self) -> &Universes {
        &self.last
    }

    #[must_use]
    pub fn stats(&self) -> Option<SenderStats> {
        self.sender.as_ref().map(DmxSender::stats)
    }
}
