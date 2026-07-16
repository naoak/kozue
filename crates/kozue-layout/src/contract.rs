use std::fmt;

use kozue_ir::{Diagram, Endpoint, Rect, Scene, SceneItem, SequenceItem};

use crate::semantic::{Point, StateEndpointId};
use crate::SemanticLayout;

/// Validated borrowed input for exchange exporters.
pub struct ExportInput<'a> {
    diagram: &'a Diagram,
    scene: &'a Scene,
    semantic: &'a SemanticLayout,
}

impl<'a> ExportInput<'a> {
    pub(crate) fn new(
        diagram: &'a Diagram,
        scene: &'a Scene,
        semantic: &'a SemanticLayout,
    ) -> Result<Self, ExportContractError> {
        validate_scene(scene)?;
        validate_contract(diagram, semantic)?;
        validate_export_semantics(semantic)?;
        validate_semantic_geometry(semantic)?;
        Ok(Self {
            diagram,
            scene,
            semantic,
        })
    }

    pub fn diagram(&self) -> &'a Diagram {
        self.diagram
    }

    pub fn scene(&self) -> &'a Scene {
        self.scene
    }

    pub fn semantic(&self) -> &'a SemanticLayout {
        self.semantic
    }
}

/// Reject future semantic enum variants before an exporter can silently map
/// them to a default presentation.
pub fn validate_export_semantics(semantic: &SemanticLayout) -> Result<(), ExportContractError> {
    let arrow = |value| match value {
        kozue_ir::ArrowType::Triangle | kozue_ir::ArrowType::None => Ok(()),
        _ => mismatch("unsupported future arrow type"),
    };
    let line = |value| match value {
        kozue_ir::LineStyle::Solid | kozue_ir::LineStyle::Dashed | kozue_ir::LineStyle::Dotted => {
            Ok(())
        }
        _ => mismatch("unsupported future line style"),
    };
    let weight = |value| match value {
        kozue_ir::LineWeight::Normal | kozue_ir::LineWeight::Thick => Ok(()),
        _ => mismatch("unsupported future line weight"),
    };
    let marker = |value| match value {
        kozue_ir::EndMarker::None
        | kozue_ir::EndMarker::HollowTriangle
        | kozue_ir::EndMarker::OpenArrow
        | kozue_ir::EndMarker::FilledDiamond
        | kozue_ir::EndMarker::HollowDiamond
        | kozue_ir::EndMarker::ErOne
        | kozue_ir::EndMarker::ErMany
        | kozue_ir::EndMarker::ErZeroOrOne
        | kozue_ir::EndMarker::ErOneOrMany
        | kozue_ir::EndMarker::ErZeroOrMany => Ok(()),
        _ => mismatch("unsupported future end marker"),
    };
    let node_kind = |value: &kozue_ir::NodeKind| match value {
        kozue_ir::NodeKind::Default
        | kozue_ir::NodeKind::Rectangle
        | kozue_ir::NodeKind::RoundedRectangle
        | kozue_ir::NodeKind::Circle
        | kozue_ir::NodeKind::Diamond => Ok(()),
        _ => mismatch("unsupported future graph node kind"),
    };
    match semantic {
        SemanticLayout::Graph(graph) => {
            for node in &graph.nodes {
                node_kind(&node.kind)?;
            }
            for edge in &graph.edges {
                arrow(edge.arrow)?;
                arrow(edge.from_arrow)?;
                line(edge.line)?;
                weight(edge.weight)?;
            }
        }
        SemanticLayout::Sequence(sequence) => {
            for message in &sequence.messages {
                arrow(message.arrow)?;
                line(message.line)?;
            }
        }
        SemanticLayout::Class(class) | SemanticLayout::Er(class) => {
            for relation in &class.relations {
                marker(relation.from_marker)?;
                marker(relation.to_marker)?;
                line(relation.line)?;
            }
        }
        SemanticLayout::State(_) => {}
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportContractError {
    message: String,
}

impl ExportContractError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ExportContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ExportContractError {}

fn mismatch(message: impl Into<String>) -> Result<(), ExportContractError> {
    Err(ExportContractError::new(message))
}

fn validate_contract(
    diagram: &Diagram,
    semantic: &SemanticLayout,
) -> Result<(), ExportContractError> {
    match (diagram, semantic) {
        (Diagram::Graph(diagram), SemanticLayout::Graph(layout)) => {
            if diagram.nodes.len() != layout.nodes.len()
                || diagram.edges.len() != layout.edges.len()
            {
                return mismatch("graph semantic element count does not match diagram");
            }
            for ((id, node), placed) in diagram.nodes.iter().zip(&layout.nodes) {
                if id != &placed.id
                    || node.id != placed.id
                    || node.label != placed.label
                    || node.kind != placed.kind
                {
                    return mismatch("graph node identity/order/semantics mismatch");
                }
            }
            for (index, (edge, placed)) in diagram.edges.iter().zip(&layout.edges).enumerate() {
                if placed.index != index
                    || edge.from != placed.from.id
                    || edge.to != placed.to.id
                    || edge.label != placed.label
                    || edge.arrow != placed.arrow
                    || edge.from_arrow != placed.from_arrow
                    || edge.line != placed.line
                    || edge.weight != placed.weight
                {
                    return mismatch("graph edge index/semantics mismatch");
                }
            }
            let expected_containers = flatten_containers(&diagram.containers);
            if expected_containers.len() != layout.containers.len() {
                return mismatch("graph container identity/order/membership mismatch");
            }
            for (expected, placed) in expected_containers.iter().zip(&layout.containers) {
                let expected_children: Vec<kozue_ir::ElementId> = expected
                    .children
                    .iter()
                    .map(|child| child.id.clone())
                    .collect();
                if expected.id != placed.id
                    || expected.label != placed.label
                    || expected.members != placed.members
                    || expected_children != placed.children
                {
                    return mismatch("graph container identity/order/membership mismatch");
                }
            }
        }
        (Diagram::Sequence(diagram), SemanticLayout::Sequence(layout)) => {
            if diagram.participants.len() != layout.participants.len() {
                return mismatch("sequence participant count does not match diagram");
            }
            for ((id, participant), placed) in diagram.participants.iter().zip(&layout.participants)
            {
                if id != &placed.id
                    || participant.id != placed.id
                    || participant.label != placed.label
                {
                    return mismatch("sequence participant identity/order mismatch");
                }
            }
            if diagram.items.len() != layout.messages.len() {
                return mismatch("sequence item/message count mismatch");
            }
            for (index, (item, placed)) in diagram.items.iter().zip(&layout.messages).enumerate() {
                let SequenceItem::Message(message) = item else {
                    return mismatch("unsupported future sequence item");
                };
                if placed.index != index
                    || message.from != placed.from
                    || message.to != placed.to
                    || message.label != placed.label
                    || message.line != placed.line
                    || message.arrow != placed.arrow
                {
                    return mismatch("sequence message index/semantics mismatch");
                }
            }
        }
        (Diagram::State(diagram), SemanticLayout::State(layout)) => {
            let mut expected_states: Vec<_> = diagram
                .states
                .iter()
                .map(|(id, state)| (id.clone(), state.label.clone()))
                .collect();
            for transition in &diagram.transitions {
                for endpoint in [&transition.from, &transition.to] {
                    if let Endpoint::State(id) = endpoint {
                        if !expected_states.iter().any(|(known, _)| known == id) {
                            expected_states.push((id.clone(), id.to_string()));
                        }
                    }
                }
            }
            if expected_states.len() != layout.states.len()
                || expected_states
                    .iter()
                    .zip(&layout.states)
                    .any(|((id, label), state)| id != &state.id || label != &state.label)
            {
                return mismatch("state identity/order mismatch");
            }
            let expects_initial = diagram
                .transitions
                .iter()
                .any(|transition| matches!(transition.from, Endpoint::Initial));
            let expects_final = diagram
                .transitions
                .iter()
                .any(|transition| matches!(transition.to, Endpoint::Final));
            if expects_initial != layout.initial.is_some()
                || expects_final != layout.final_state.is_some()
            {
                return mismatch("state pseudostate presence mismatch");
            }
            if diagram.transitions.len() != layout.transitions.len() {
                return mismatch("state transition count mismatch");
            }
            for (index, (transition, placed)) in diagram
                .transitions
                .iter()
                .zip(&layout.transitions)
                .enumerate()
            {
                if placed.index != index
                    || !state_endpoint_matches(&transition.from, &placed.from)
                    || !state_endpoint_matches(&transition.to, &placed.to)
                    || transition.label != placed.label
                {
                    return mismatch("state transition index/semantics mismatch");
                }
            }
        }
        (Diagram::Class(diagram), SemanticLayout::Class(layout)) => {
            if diagram.classes.len() != layout.boxes.len()
                || diagram.relations.len() != layout.relations.len()
            {
                return mismatch("class semantic element count does not match diagram");
            }
            for ((id, class), placed) in diagram.classes.iter().zip(&layout.boxes) {
                let expected_compartments: Vec<&Vec<String>> = [&class.attributes, &class.methods]
                    .into_iter()
                    .filter(|rows| !rows.is_empty())
                    .collect();
                if id != &placed.id
                    || class.id != placed.id
                    || class.name != placed.title
                    || class.stereotype != placed.stereotype
                    || expected_compartments.len() != placed.compartments.len()
                    || expected_compartments
                        .iter()
                        .zip(&placed.compartments)
                        .any(|(rows, compartment)| *rows != &compartment.rows)
                {
                    return mismatch("class identity/order mismatch");
                }
            }
            for (index, (relation, placed)) in
                diagram.relations.iter().zip(&layout.relations).enumerate()
            {
                if placed.index != index
                    || relation.from != placed.from
                    || relation.to != placed.to
                    || relation.from_marker != placed.from_marker
                    || relation.to_marker != placed.to_marker
                    || relation.line != placed.line
                    || relation.label != placed.label
                    || relation.from_mult != placed.from_mult
                    || relation.to_mult != placed.to_mult
                {
                    return mismatch("class relation index/semantics mismatch");
                }
            }
        }
        (Diagram::Er(diagram), SemanticLayout::Er(layout)) => {
            if diagram.entities.len() != layout.boxes.len()
                || diagram.relations.len() != layout.relations.len()
            {
                return mismatch("ER semantic element count does not match diagram");
            }
            for ((id, entity), placed) in diagram.entities.iter().zip(&layout.boxes) {
                let expected_rows: Vec<String> =
                    entity.attributes.iter().map(format_attr).collect();
                let actual_rows = placed
                    .compartments
                    .first()
                    .map(|compartment| compartment.rows.as_slice())
                    .unwrap_or_default();
                if id != &placed.id
                    || entity.id != placed.id
                    || entity.name != placed.title
                    || placed.stereotype.is_some()
                    || expected_rows != actual_rows
                    || placed.compartments.len() != usize::from(!expected_rows.is_empty())
                {
                    return mismatch("ER identity/order mismatch");
                }
            }
            for (index, (relation, placed)) in
                diagram.relations.iter().zip(&layout.relations).enumerate()
            {
                if placed.index != index
                    || relation.from != placed.from
                    || relation.to != placed.to
                    || relation.from_marker != placed.from_marker
                    || relation.to_marker != placed.to_marker
                    || relation.line != placed.line
                    || relation.label != placed.label
                {
                    return mismatch("ER relation index/semantics mismatch");
                }
            }
        }
        _ => return mismatch("diagram and semantic layout variants do not match"),
    }
    Ok(())
}

/// Flatten a container tree into pre-order (root, then each child
/// recursively, in declaration order) — matching the order
/// [`crate::layout_graph_full`] builds `GraphLayout::containers` in.
fn flatten_containers(containers: &[kozue_ir::Container]) -> Vec<&kozue_ir::Container> {
    let mut out = Vec::new();
    for c in containers {
        flatten_containers_into(c, &mut out);
    }
    out
}

fn flatten_containers_into<'a>(
    container: &'a kozue_ir::Container,
    out: &mut Vec<&'a kozue_ir::Container>,
) {
    out.push(container);
    for child in &container.children {
        flatten_containers_into(child, out);
    }
}

fn format_attr(attribute: &kozue_ir::ErAttribute) -> String {
    let mut formatted = String::new();
    if !attribute.keys.is_empty() {
        formatted.push('[');
        formatted.push_str(&attribute.keys.join(","));
        formatted.push_str("] ");
    }
    formatted.push_str(&attribute.name);
    if !attribute.type_name.is_empty() {
        formatted.push_str(": ");
        formatted.push_str(&attribute.type_name);
    }
    if let Some(comment) = &attribute.comment {
        formatted.push_str("  // ");
        formatted.push_str(comment);
    }
    formatted
}

fn state_endpoint_matches(endpoint: &Endpoint, placed: &StateEndpointId) -> bool {
    match (endpoint, placed) {
        (Endpoint::Initial, StateEndpointId::Initial)
        | (Endpoint::Final, StateEndpointId::Final) => true,
        (Endpoint::State(left), StateEndpointId::State(right)) => left == right,
        _ => false,
    }
}

fn valid(values: &[f64]) -> bool {
    values
        .iter()
        .all(|value| value.is_finite() && *value >= 0.0)
}

fn validate_rect(rect: &Rect) -> Result<(), ExportContractError> {
    if valid(&[rect.x, rect.y, rect.width, rect.height, rect.rx]) {
        Ok(())
    } else {
        mismatch("export geometry must be finite and nonnegative")
    }
}

fn validate_point(point: &Point) -> Result<(), ExportContractError> {
    if valid(&[point.x, point.y]) {
        Ok(())
    } else {
        mismatch("export geometry must be finite and nonnegative")
    }
}

fn validate_scene(scene: &Scene) -> Result<(), ExportContractError> {
    if !valid(&[scene.width, scene.height]) {
        return mismatch("scene bounds must be finite and nonnegative");
    }
    fn walk_items(scene_items: &[SceneItem]) -> Result<(), ExportContractError> {
        for item in scene_items {
            match item {
                SceneItem::Rect(rect) => validate_rect(rect)?,
                SceneItem::Path(path) => {
                    if path.points.len() < 2 {
                        return mismatch("scene paths require at least two points");
                    }
                    for &(x, y) in &path.points {
                        if !valid(&[x, y]) {
                            return mismatch("scene path must be finite and nonnegative");
                        }
                    }
                }
                SceneItem::Text(text) => {
                    if !valid(&[text.x, text.y, text.size, text.text_width, text.text_height]) {
                        return mismatch("scene text geometry must be finite and nonnegative");
                    }
                }
                SceneItem::Group(group) => walk_items(&group.items)?,
                _ => return mismatch("unsupported future scene item"),
            }
        }
        Ok(())
    }
    walk_items(&scene.items)
}

fn validate_semantic_geometry(layout: &SemanticLayout) -> Result<(), ExportContractError> {
    let points = |route: &[Point]| {
        if route.len() < 2 {
            return mismatch("semantic routes require at least two points");
        }
        route.iter().try_for_each(validate_point)
    };
    let label_anchor = |label: &Option<String>, anchor: &Option<Point>| {
        if label.is_some() != anchor.is_some() {
            mismatch("semantic label and label anchor presence must match")
        } else if let Some(anchor) = anchor {
            validate_point(anchor)
        } else {
            Ok(())
        }
    };
    match layout {
        SemanticLayout::Graph(graph) => {
            for node in &graph.nodes {
                validate_rect(&node.rect)?;
                validate_point(&node.label_anchor)?;
            }
            for edge in &graph.edges {
                points(&edge.route)?;
                label_anchor(&edge.label, &edge.label_anchor)?;
            }
            for container in &graph.containers {
                validate_rect(&container.rect)?;
                label_anchor(&container.label, &container.label_anchor)?;
            }
        }
        SemanticLayout::Sequence(sequence) => {
            for participant in &sequence.participants {
                validate_rect(&participant.header_rect)?;
                if !valid(&[
                    participant.lifeline_x,
                    participant.lifeline_y0,
                    participant.lifeline_y1,
                ]) {
                    return mismatch("sequence geometry must be finite and nonnegative");
                }
            }
            for message in &sequence.messages {
                points(&message.route)?;
                label_anchor(&message.label, &message.label_anchor)?;
            }
        }
        SemanticLayout::State(state) => {
            for node in &state.states {
                validate_rect(&node.rect)?;
                validate_point(&node.label_anchor)?;
            }
            if let Some(initial) = &state.initial {
                validate_point(&initial.center)?;
                if !valid(&[initial.radius]) {
                    return mismatch("state geometry must be finite and nonnegative");
                }
            }
            if let Some(final_state) = &state.final_state {
                validate_point(&final_state.center)?;
                if !valid(&[final_state.inner_radius, final_state.outer_radius]) {
                    return mismatch("state geometry must be finite and nonnegative");
                }
            }
            for transition in &state.transitions {
                points(&transition.route)?;
                label_anchor(&transition.label, &transition.label_anchor)?;
            }
        }
        SemanticLayout::Class(class) | SemanticLayout::Er(class) => {
            if !valid(&[class.width, class.height]) {
                return mismatch("box layout bounds must be finite and nonnegative");
            }
            for boxed in &class.boxes {
                validate_rect(&boxed.rect)?;
                for compartment in &boxed.compartments {
                    if !valid(&[compartment.top_y]) {
                        return mismatch("compartment geometry must be finite and nonnegative");
                    }
                }
            }
            for relation in &class.relations {
                if relation.points.len() < 2 {
                    return mismatch("semantic relation routes require at least two points");
                }
                for &(x, y) in &relation.points {
                    if !valid(&[x, y]) {
                        return mismatch("relation geometry must be finite and nonnegative");
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use kozue_ir::{
        ArrowType, ClassDiagram, ClassNode, ClassRelation, Diagram, Direction, EndMarker, Endpoint,
        ErAttribute, ErDiagram, ErEntity, ErRelation, GraphDiagram, LineStyle, LineWeight, Message,
        Node, Participant, SceneItem, SequenceDiagram, SequenceItem, State, StateDiagram,
        Transition,
    };

    use crate::{layout_full, SemanticLayout};

    fn fixtures() -> Vec<(Diagram, Diagram)> {
        let mut graph = GraphDiagram::new(Direction::Down);
        graph.nodes.insert("g".into(), Node::new("g", "Graph"));

        let mut sequence = SequenceDiagram::new();
        sequence
            .participants
            .insert("p".into(), Participant::new("p", "Participant"));

        let mut state = StateDiagram::new();
        state.states.insert("s".into(), State::new("s", "State"));

        let mut class = ClassDiagram::new(Direction::Down);
        class
            .classes
            .insert("c".into(), ClassNode::new("c", "Class"));

        let mut er = ErDiagram::new();
        er.entities.insert("e".into(), ErEntity::new("e", "Entity"));

        vec![
            (
                Diagram::Graph(graph),
                Diagram::Graph(GraphDiagram::new(Direction::Down)),
            ),
            (
                Diagram::Sequence(sequence),
                Diagram::Sequence(SequenceDiagram::new()),
            ),
            (Diagram::State(state), Diagram::State(StateDiagram::new())),
            (
                Diagram::Class(class),
                Diagram::Class(ClassDiagram::new(Direction::Down)),
            ),
            (Diagram::Er(er), Diagram::Er(ErDiagram::new())),
        ]
    }

    #[test]
    fn export_input_accepts_all_five_domains_and_exposes_original_borrows() {
        for (diagram, _) in fixtures() {
            let output = layout_full(&diagram).unwrap();
            let input = output.export_input(&diagram).unwrap();
            assert!(std::ptr::eq(input.diagram(), &diagram));
            assert!(std::ptr::eq(input.scene(), &output.scene));
            assert!(std::ptr::eq(input.semantic(), &output.semantic));
        }
    }

    #[test]
    fn export_input_rejects_semantic_mismatch_in_all_five_domains() {
        for (diagram, mismatched) in fixtures() {
            let output = layout_full(&diagram).unwrap();
            assert!(output.export_input(&mismatched).is_err());
        }
    }

    #[test]
    fn export_input_rejects_variant_and_nonfinite_geometry_mismatches() {
        let (diagram, _) = fixtures().remove(0);
        let mut output = layout_full(&diagram).unwrap();
        let wrong = Diagram::Sequence(SequenceDiagram::new());
        assert!(output.export_input(&wrong).is_err());
        output.scene.width = f64::NAN;
        assert!(output.export_input(&diagram).is_err());
    }

    fn graph_with_edge(label: Option<&str>) -> Diagram {
        let mut graph = GraphDiagram::new(Direction::Down);
        graph.nodes.insert("a".into(), Node::new("a", "A"));
        graph.nodes.insert("b".into(), Node::new("b", "B"));
        graph.edges.push(kozue_ir::Edge::new(
            "a",
            "b",
            label.map(str::to_string),
            ArrowType::Triangle,
        ));
        Diagram::Graph(graph)
    }

    fn sequence_with_message(label: Option<&str>) -> Diagram {
        let mut sequence = SequenceDiagram::new();
        sequence
            .participants
            .insert("a".into(), Participant::new("a", "A"));
        sequence
            .participants
            .insert("b".into(), Participant::new("b", "B"));
        sequence.items.push(SequenceItem::Message(Message::new(
            "a",
            "b",
            label.map(str::to_string),
            LineStyle::Solid,
            ArrowType::Triangle,
        )));
        Diagram::Sequence(sequence)
    }

    fn state_with_transition(label: Option<&str>) -> Diagram {
        let mut state = StateDiagram::new();
        state.states.insert("a".into(), State::new("a", "A"));
        state.states.insert("b".into(), State::new("b", "B"));
        state.transitions.push(Transition::new(
            Endpoint::State("a".into()),
            Endpoint::State("b".into()),
            label.map(str::to_string),
        ));
        Diagram::State(state)
    }

    #[test]
    fn export_input_rejects_empty_and_singleton_routes_and_scene_paths() {
        let graph = graph_with_edge(None);
        for point_count in [0, 1] {
            let mut output = layout_full(&graph).unwrap();
            let SemanticLayout::Graph(layout) = &mut output.semantic else {
                unreachable!()
            };
            layout.edges[0].route.truncate(point_count);
            assert!(output.export_input(&graph).is_err());
        }

        let sequence = sequence_with_message(None);
        for point_count in [0, 1] {
            let mut output = layout_full(&sequence).unwrap();
            let SemanticLayout::Sequence(layout) = &mut output.semantic else {
                unreachable!()
            };
            layout.messages[0].route.truncate(point_count);
            assert!(output.export_input(&sequence).is_err());
        }

        let state = state_with_transition(None);
        for point_count in [0, 1] {
            let mut output = layout_full(&state).unwrap();
            let SemanticLayout::State(layout) = &mut output.semantic else {
                unreachable!()
            };
            layout.transitions[0].route.truncate(point_count);
            assert!(output.export_input(&state).is_err());
        }

        let mut class = ClassDiagram::new(Direction::Down);
        class.classes.insert("a".into(), ClassNode::new("a", "A"));
        class.classes.insert("b".into(), ClassNode::new("b", "B"));
        class.relations.push(ClassRelation::new(
            "a",
            "b",
            EndMarker::None,
            EndMarker::OpenArrow,
            LineStyle::Solid,
            None,
            None,
            None,
        ));
        let class = Diagram::Class(class);
        for point_count in [0, 1] {
            let mut output = layout_full(&class).unwrap();
            let SemanticLayout::Class(layout) = &mut output.semantic else {
                unreachable!()
            };
            layout.relations[0].points.truncate(point_count);
            assert!(output.export_input(&class).is_err());
        }

        let mut er = ErDiagram::new();
        er.entities.insert("a".into(), ErEntity::new("a", "A"));
        er.entities.insert("b".into(), ErEntity::new("b", "B"));
        er.relations.push(ErRelation::new(
            "a",
            "b",
            EndMarker::ErOne,
            EndMarker::ErMany,
            None,
            LineStyle::Solid,
        ));
        let er = Diagram::Er(er);
        for point_count in [0, 1] {
            let mut output = layout_full(&er).unwrap();
            let SemanticLayout::Er(layout) = &mut output.semantic else {
                unreachable!()
            };
            layout.relations[0].points.truncate(point_count);
            assert!(output.export_input(&er).is_err());
        }

        for point_count in [0, 1] {
            let mut output = layout_full(&graph).unwrap();
            let path = output
                .scene
                .items
                .iter_mut()
                .find_map(|item| match item {
                    SceneItem::Path(path) => Some(path),
                    _ => None,
                })
                .unwrap();
            path.points.truncate(point_count);
            assert!(output.export_input(&graph).is_err());
        }
    }

    #[test]
    fn export_input_rejects_missing_and_stray_label_anchors() {
        for diagram in [
            graph_with_edge(Some("label")),
            sequence_with_message(Some("label")),
            state_with_transition(Some("label")),
        ] {
            let mut missing = layout_full(&diagram).unwrap();
            match &mut missing.semantic {
                SemanticLayout::Graph(layout) => layout.edges[0].label_anchor = None,
                SemanticLayout::Sequence(layout) => layout.messages[0].label_anchor = None,
                SemanticLayout::State(layout) => layout.transitions[0].label_anchor = None,
                _ => unreachable!(),
            }
            assert!(missing.export_input(&diagram).is_err());
        }

        for diagram in [
            graph_with_edge(None),
            sequence_with_message(None),
            state_with_transition(None),
        ] {
            let mut stray = layout_full(&diagram).unwrap();
            match &mut stray.semantic {
                SemanticLayout::Graph(layout) => {
                    layout.edges[0].label_anchor = Some(crate::semantic::Point::new(1.0, 1.0))
                }
                SemanticLayout::Sequence(layout) => {
                    layout.messages[0].label_anchor = Some(crate::semantic::Point::new(1.0, 1.0))
                }
                SemanticLayout::State(layout) => {
                    layout.transitions[0].label_anchor = Some(crate::semantic::Point::new(1.0, 1.0))
                }
                _ => unreachable!(),
            }
            assert!(stray.export_input(&diagram).is_err());
        }
    }

    #[test]
    fn export_input_rejects_mutated_derived_domain_semantics() {
        let mut state = StateDiagram::new();
        state.transitions.push(Transition::new(
            Endpoint::Initial,
            Endpoint::State("auto".into()),
            None,
        ));
        state.transitions.push(Transition::new(
            Endpoint::State("auto".into()),
            Endpoint::Final,
            None,
        ));
        let state = Diagram::State(state);
        let mut state_label = layout_full(&state).unwrap();
        let SemanticLayout::State(layout) = &mut state_label.semantic else {
            unreachable!()
        };
        layout.states[0].label = "wrong".into();
        assert!(state_label.export_input(&state).is_err());
        let mut pseudostate = layout_full(&state).unwrap();
        let SemanticLayout::State(layout) = &mut pseudostate.semantic else {
            unreachable!()
        };
        layout.initial = None;
        assert!(pseudostate.export_input(&state).is_err());

        let mut class = ClassDiagram::new(Direction::Down);
        let mut class_node = ClassNode::new("c", "Class");
        class_node.stereotype = Some("interface".into());
        class_node.attributes.push("+value: String".into());
        class.classes.insert("c".into(), class_node);
        let class = Diagram::Class(class);
        let mut stereotype = layout_full(&class).unwrap();
        let SemanticLayout::Class(layout) = &mut stereotype.semantic else {
            unreachable!()
        };
        layout.boxes[0].stereotype = Some("abstract".into());
        assert!(stereotype.export_input(&class).is_err());
        let mut compartment = layout_full(&class).unwrap();
        let SemanticLayout::Class(layout) = &mut compartment.semantic else {
            unreachable!()
        };
        layout.boxes[0].compartments[0].rows[0] = "wrong".into();
        assert!(compartment.export_input(&class).is_err());

        let mut er = ErDiagram::new();
        let mut entity = ErEntity::new("e", "Entity");
        entity.attributes.push(ErAttribute::new(
            "uuid",
            "id",
            vec!["PK".into()],
            Some("identifier".into()),
        ));
        er.entities.insert("e".into(), entity);
        let er = Diagram::Er(er);
        let mut row = layout_full(&er).unwrap();
        let SemanticLayout::Er(layout) = &mut row.semantic else {
            unreachable!()
        };
        layout.boxes[0].compartments[0].rows[0] = "wrong".into();
        assert!(row.export_input(&er).is_err());
    }

    #[test]
    fn export_input_rejects_graph_edge_presentation_field_mismatches() {
        let graph = graph_with_edge(None);
        let mut from_arrow = layout_full(&graph).unwrap();
        let SemanticLayout::Graph(layout) = &mut from_arrow.semantic else {
            unreachable!()
        };
        layout.edges[0].from_arrow = ArrowType::Triangle;
        assert!(from_arrow.export_input(&graph).is_err());

        let mut line = layout_full(&graph).unwrap();
        let SemanticLayout::Graph(layout) = &mut line.semantic else {
            unreachable!()
        };
        layout.edges[0].line = LineStyle::Dashed;
        assert!(line.export_input(&graph).is_err());

        let mut weight = layout_full(&graph).unwrap();
        let SemanticLayout::Graph(layout) = &mut weight.semantic else {
            unreachable!()
        };
        layout.edges[0].weight = LineWeight::Thick;
        assert!(weight.export_input(&graph).is_err());
    }

    fn graph_with_container() -> Diagram {
        let mut graph = GraphDiagram::new(Direction::Down);
        graph.nodes.insert("a".into(), Node::new("a", "A"));
        let mut container = kozue_ir::Container::new("x", Some("X".to_string()));
        container.members.push("a".into());
        graph.containers.push(container);
        Diagram::Graph(graph)
    }

    #[test]
    fn export_input_rejects_container_count_mismatch() {
        let graph = graph_with_container();
        let mut output = layout_full(&graph).unwrap();
        let SemanticLayout::Graph(layout) = &mut output.semantic else {
            unreachable!()
        };
        layout.containers.clear();
        assert!(output.export_input(&graph).is_err());
    }

    #[test]
    fn export_input_rejects_container_id_mismatch() {
        let graph = graph_with_container();
        let mut output = layout_full(&graph).unwrap();
        let SemanticLayout::Graph(layout) = &mut output.semantic else {
            unreachable!()
        };
        layout.containers[0].id = "wrong".into();
        assert!(output.export_input(&graph).is_err());
    }

    #[test]
    fn export_input_rejects_container_nan_rect() {
        let graph = graph_with_container();
        let mut output = layout_full(&graph).unwrap();
        let SemanticLayout::Graph(layout) = &mut output.semantic else {
            unreachable!()
        };
        layout.containers[0].rect.x = f64::NAN;
        assert!(output.export_input(&graph).is_err());
    }

    #[test]
    fn export_input_rejects_container_label_anchor_parity_violation() {
        let graph = graph_with_container();

        let mut missing_anchor = layout_full(&graph).unwrap();
        let SemanticLayout::Graph(layout) = &mut missing_anchor.semantic else {
            unreachable!()
        };
        layout.containers[0].label_anchor = None;
        assert!(missing_anchor.export_input(&graph).is_err());

        let mut stray_anchor = layout_full(&graph).unwrap();
        let SemanticLayout::Graph(layout) = &mut stray_anchor.semantic else {
            unreachable!()
        };
        layout.containers[0].label = None;
        assert!(stray_anchor.export_input(&graph).is_err());
    }
}
