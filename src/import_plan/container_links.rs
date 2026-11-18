use serde::Serialize;

use crate::experimental::restore_rules::DynamicItemPropertyKind;
use crate::experimental::tagged::{TagKind, TagProfile, is_name, read_framed_header};
use crate::experimental::typed_value::{Scalar, TypedProperty, TypedTree, Value};
use crate::import_candidate::DecodedRecorder;

use super::ContainerTopology;

const CHARACTER: &str = "/Script/Icarus.PlayerStateRecorderComponent";
const PROFILE: TagProfile = TagProfile::MemoryArchive522;
const LINKED_INVENTORY: DynamicItemPropertyKind =
    DynamicItemPropertyKind::InventoryContainerLinkedInventoryId;

/// Counts of saved item records, plus scopes whose items could not be assessed.
/// Items without a property-13 entry do not contribute to the counts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ContainerLinkPlan {
    /// The explicit property-13 value was -1.
    pub no_link: usize,
    /// The value identified a saved entry within the single manager's list.
    pub occupied: usize,
    /// The value fell within that list at an omitted position.
    pub unoccupied: usize,
    /// The value was nonnegative and beyond that list.
    pub out_of_range: usize,
    /// The item's property-13 data or the nonnegative target could not be qualified.
    pub unknown_items: usize,
    /// The recorder or its saved inventory array could not be assessed.
    pub unknown_recorders: usize,
    /// The inventory's slots could not be assessed.
    pub unknown_inventories: usize,
}

pub(super) fn assess(
    recorders: &[DecodedRecorder],
    topology: Option<&ContainerTopology>,
) -> Option<ContainerLinkPlan> {
    let mut assessment = None;
    for recorder in recorders.iter().filter(|recorder| {
        recorder.decision().retained && is_name(recorder.class_units(), CHARACTER)
    }) {
        let plan = assessment.get_or_insert_with(ContainerLinkPlan::default);
        let Some(inventories) = recorder.typing().lossless().and_then(|(typed, _)| {
            struct_elements(&typed.tree, "SavedInventories", "InventorySaveData").ok()
        }) else {
            plan.unknown_recorders += 1;
            continue;
        };
        for inventory in inventories {
            let Ok(items) = struct_elements(inventory, "Slots", "InventorySlotSaveData") else {
                plan.unknown_inventories += 1;
                continue;
            };
            for item in items {
                match (associated_position(item), topology) {
                    (Ok(None), _) => {}
                    (Ok(Some(-1)), _) => plan.no_link += 1,
                    (Ok(Some(position)), Some(topology)) if position >= 0 => {
                        if position >= topology.length {
                            plan.out_of_range += 1;
                        } else if topology.occupied.contains(&position) {
                            plan.occupied += 1;
                        } else {
                            plan.unoccupied += 1;
                        }
                    }
                    _ => plan.unknown_items += 1,
                }
            }
        }
    }
    assessment
}

fn associated_position(item: &TypedTree) -> Result<Option<i32>, ()> {
    let mut position = None;
    for entry in struct_elements(item, "DynamicData", "InventorySlotDynamicData")? {
        let index = explicit_int(entry, "Index")?;
        if DynamicItemPropertyKind::from_saved_index(index) == Some(LINKED_INVENTORY) {
            // The restore reader narrows to a byte. Only a single explicit
            // selector 13 is qualified for this observation.
            if index != LINKED_INVENTORY as i32 || position.is_some() {
                return Err(());
            }
            position = Some(explicit_int(entry, "Value")?);
        }
    }
    Ok(position)
}

fn unique_property<'a>(tree: &'a TypedTree, name: &str) -> Result<Option<&'a TypedProperty>, ()> {
    let mut matching = tree
        .properties
        .iter()
        .filter(|property| is_name(&property.name, name));
    let property = matching.next();
    if matching.next().is_some() {
        return Err(());
    }
    Ok(property)
}

fn explicit_int(tree: &TypedTree, name: &str) -> Result<i32, ()> {
    let property = unique_property(tree, name)?.ok_or(())?;
    let header = read_framed_header(&property.header, PROFILE).map_err(|_| ())?;
    match (header.kind, header.index, &property.value) {
        (TagKind::Int, 0, Value::Scalar(Scalar::Int(value))) => Ok(*value),
        _ => Err(()),
    }
}

/// Borrow only the arrays on the path to the item link. The general save
/// readers accept a typed array by name without qualifying its shared header.
fn struct_elements<'a>(
    tree: &'a TypedTree,
    name: &str,
    element_type: &str,
) -> Result<&'a [TypedTree], ()> {
    let Some(property) = unique_property(tree, name)? else {
        return Ok(&[]);
    };
    let Value::StructArray {
        shared_header,
        elements,
        ..
    } = &property.value
    else {
        return Err(());
    };
    let outer = read_framed_header(&property.header, PROFILE).map_err(|_| ())?;
    let shared = read_framed_header(shared_header, PROFILE).map_err(|_| ())?;
    if outer.kind != TagKind::Array
        || outer.index != 0
        || !outer
            .extra
            .as_deref()
            .is_some_and(|name| is_name(name, "StructProperty"))
        || shared.kind != TagKind::Struct
        || shared.index != 0
        || !is_name(&shared.name, name)
        || !shared
            .extra
            .as_deref()
            .is_some_and(|name| is_name(name, element_type))
    {
        return Err(());
    }
    Ok(elements)
}
