use crate::osm_parser::{ProcessedMember, ProcessedMemberRole, ProcessedNode, ProcessedWay};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::sync::Mutex;

lazy_static::lazy_static! {
    static ref DEBUG_LOG: Mutex<DebugLogger> = Mutex::new(DebugLogger::new());
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub id: u64,
    pub x: i32,
    pub z: i32,
    pub tags: HashMap<String, String>,
}

impl From<&ProcessedNode> for NodeSnapshot {
    fn from(node: &ProcessedNode) -> Self {
        NodeSnapshot {
            id: node.id,
            x: node.x,
            z: node.z,
            tags: node.tags.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaySnapshot {
    pub id: u64,
    pub tags: HashMap<String, String>,
    pub nodes: Vec<NodeSnapshot>,
    pub node_count: usize,
}

impl From<&ProcessedWay> for WaySnapshot {
    fn from(way: &ProcessedWay) -> Self {
        WaySnapshot {
            id: way.id,
            tags: way.tags.clone(),
            nodes: way.nodes.iter().map(NodeSnapshot::from).collect(),
            node_count: way.nodes.len(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemberSnapshot {
    pub role: String,
    pub way: WaySnapshot,
}

impl From<&ProcessedMember> for MemberSnapshot {
    fn from(member: &ProcessedMember) -> Self {
        MemberSnapshot {
            role: match member.role {
                ProcessedMemberRole::Outer => "outer".to_string(),
                ProcessedMemberRole::Inner => "inner".to_string(),
            },
            way: WaySnapshot::from(&member.way),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelationSnapshot {
    pub id: u64,
    pub tags: HashMap<String, String>,
    pub members: Vec<MemberSnapshot>,
    pub member_count: usize,
    pub total_nodes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformationStage {
    pub stage: String,
    pub timestamp: String,
    pub element_type: String,
    pub element_id: u64,
    pub way: Option<WaySnapshot>,
    pub relation: Option<RelationSnapshot>,
    pub notes: Vec<String>,
}

pub struct DebugLogger {
    enabled_elements: Vec<u64>,
    stages: Vec<TransformationStage>,
}

impl DebugLogger {
    pub fn new() -> Self {
        // Read element IDs from environment variable ARNIS_DEBUG_ELEMENTS
        // Format: ARNIS_DEBUG_ELEMENTS=6553308,1470430,18254505
        let enabled_elements: Vec<u64> = std::env::var("ARNIS_DEBUG_ELEMENTS")
            .ok()
            .map(|s| {
                s.split(',')
                    .filter_map(|id| id.trim().parse::<u64>().ok())
                    .collect()
            })
            .unwrap_or_default();

        if !enabled_elements.is_empty() {
            eprintln!(
                "DEBUG: Element tracking enabled for IDs: {:?}",
                enabled_elements
            );
        }

        DebugLogger {
            enabled_elements,
            stages: Vec::new(),
        }
    }

    pub fn is_tracking(&self, element_id: u64) -> bool {
        !self.enabled_elements.is_empty() && self.enabled_elements.contains(&element_id)
    }

    pub fn log_way(&mut self, stage: &str, way: &ProcessedWay, notes: Vec<String>) {
        if !self.is_tracking(way.id) {
            return;
        }

        self.stages.push(TransformationStage {
            stage: stage.to_string(),
            timestamp: chrono::Local::now().format("%H:%M:%S%.3f").to_string(),
            element_type: "way".to_string(),
            element_id: way.id,
            way: Some(WaySnapshot::from(way)),
            relation: None,
            notes,
        });
    }

    pub fn log_relation(
        &mut self,
        stage: &str,
        relation_id: u64,
        tags: &HashMap<String, String>,
        members: &[ProcessedMember],
        notes: Vec<String>,
    ) {
        if !self.is_tracking(relation_id) {
            return;
        }

        let total_nodes: usize = members.iter().map(|m| m.way.nodes.len()).sum();

        self.stages.push(TransformationStage {
            stage: stage.to_string(),
            timestamp: chrono::Local::now().format("%H:%M:%S%.3f").to_string(),
            element_type: "relation".to_string(),
            element_id: relation_id,
            way: None,
            relation: Some(RelationSnapshot {
                id: relation_id,
                tags: tags.clone(),
                members: members.iter().map(MemberSnapshot::from).collect(),
                member_count: members.len(),
                total_nodes,
            }),
            notes,
        });
    }

    pub fn write_to_file(&self) -> std::io::Result<()> {
        if self.stages.is_empty() {
            return Ok(());
        }

        let filename = format!(
            "debug_elements_{}.json",
            chrono::Local::now().format("%Y%m%d_%H%M%S")
        );

        let mut file = File::create(&filename)?;
        let json = serde_json::to_string_pretty(&self.stages)?;
        file.write_all(json.as_bytes())?;

        eprintln!(
            "DEBUG: Wrote {} transformation stages to {}",
            self.stages.len(),
            filename
        );
        Ok(())
    }
}

// Public API functions
pub fn log_way_transformation(stage: &str, way: &ProcessedWay, notes: Vec<String>) {
    if let Ok(mut logger) = DEBUG_LOG.lock() {
        logger.log_way(stage, way, notes);
    }
}

pub fn log_relation_transformation(
    stage: &str,
    relation_id: u64,
    tags: &HashMap<String, String>,
    members: &[ProcessedMember],
    notes: Vec<String>,
) {
    if let Ok(mut logger) = DEBUG_LOG.lock() {
        logger.log_relation(stage, relation_id, tags, members, notes);
    }
}

pub fn is_tracking_element(element_id: u64) -> bool {
    DEBUG_LOG
        .lock()
        .map(|logger| logger.is_tracking(element_id))
        .unwrap_or(false)
}

pub fn write_debug_log() {
    if let Ok(logger) = DEBUG_LOG.lock() {
        if let Err(e) = logger.write_to_file() {
            eprintln!("ERROR: Failed to write debug log: {}", e);
        }
    }
}
