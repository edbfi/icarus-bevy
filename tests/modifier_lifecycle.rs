use icarus_bevy::experimental::modifier_lifecycle::{
    FixModifierActivationAction, FixModifierActivationCandidate, FixModifierActivationGuards,
    MODIFIER_AUTHORITY_NET_ROLE, MODIFIER_FIX_ACTIVATION_STATE,
    MODIFIER_HIGH_FREQUENCY_TICK_RATE_BITS, MODIFIER_INVALID_STAT_ENUM,
    MODIFIER_SCENE_ATTACH_KEEP_RELATIVE_PACKED, MODIFIER_STAT_SCALE_CENTI_BITS,
    MODIFIER_STAT_SOURCE_KIND, MODIFIER_TIMER_DEFAULT_FIRST_DELAY, ModifierActivateOutcome,
    ModifierAppliedOutcome, ModifierBeginPlayOutcome, ModifierComparatorOperations,
    ModifierComparatorScratchSlot, ModifierDeactivateOutcome, ModifierEndPlayInputs,
    ModifierEndPlayOutcome, ModifierInitComponentInputs, ModifierInitComponentOutcome,
    ModifierLoopingTimerSetup, ModifierOnStatContainerUpdatedInputs,
    ModifierOnStatContainerUpdatedOutcome, ModifierPrimaryVftableSlot, ModifierRemovedOutcome,
    ModifierSceneComponentInitAction, ModifierStateDataSource, ModifierTimerClearTarget,
    ModifierUpdateStatsOutcome, RemoveModifierStateCandidate, RemoveModifierStateSelection,
    RemoveModifierStateSelectionOutcome, RemoveModifierStateVirtualCallRequest,
    SiblingModifierGrantedTags, evaluate_modifier_activate, evaluate_modifier_applied_impl,
    evaluate_modifier_begin_play, evaluate_modifier_comparator_local, evaluate_modifier_deactivate,
    evaluate_modifier_end_play, evaluate_modifier_get_state_data,
    evaluate_modifier_init_component_impl, evaluate_modifier_on_stat_container_updated,
    evaluate_modifier_removed_impl, evaluate_modifier_update_stats,
    resolve_modifier_primary_vftable_slot, scale_modifier_granted_stat_value,
    select_fix_modifier_activation_actions, select_remove_modifier_state_local,
};

#[test]
fn modifier_primary_vftable_slots_match_cross_build_table() {
    let expected = [
        (0x220, ModifierPrimaryVftableSlot::UObjectProcessEvent),
        (
            0x270,
            ModifierPrimaryVftableSlot::ActorComponentOnRepIsActive,
        ),
        (0x278, ModifierPrimaryVftableSlot::ModifierActivate),
        (0x280, ModifierPrimaryVftableSlot::ModifierDeactivate),
        (0x288, ModifierPrimaryVftableSlot::ActorComponentSetActive),
        (0x338, ModifierPrimaryVftableSlot::ModifierBeginPlay),
        (0x340, ModifierPrimaryVftableSlot::ModifierEndPlay),
        (
            0x3c0,
            ModifierPrimaryVftableSlot::ActorComponentDestroyComponent,
        ),
        (0x400, ModifierPrimaryVftableSlot::IcfReturnFalseStub400),
        (0x408, ModifierPrimaryVftableSlot::ModifierUpdateStats),
        (
            0x410,
            ModifierPrimaryVftableSlot::ModifierAppliedImplementation,
        ),
        (
            0x418,
            ModifierPrimaryVftableSlot::ModifierRemovedImplementation,
        ),
        (0x420, ModifierPrimaryVftableSlot::IcfVoidNoOpStub420),
        (0x428, ModifierPrimaryVftableSlot::IcfReturnTrueStub428),
        (
            0x430,
            ModifierPrimaryVftableSlot::ModifierInitComponentImplementation,
        ),
        (0x438, ModifierPrimaryVftableSlot::IcfReturnTrueStub438),
    ];
    for (offset, slot) in expected {
        assert_eq!(resolve_modifier_primary_vftable_slot(offset), Some(slot));
    }
    assert_eq!(resolve_modifier_primary_vftable_slot(0x000), None);
    assert_eq!(resolve_modifier_primary_vftable_slot(0x3f8), None);
    assert_eq!(resolve_modifier_primary_vftable_slot(0x440), None);
}

#[test]
fn modifier_deactivate_activate_and_update_stats_rules_cover_all_branches() {
    // Deactivate (+0x280)
    assert_eq!(
        evaluate_modifier_deactivate(false, true, 42),
        ModifierDeactivateOutcome {
            run_base_deactivate: true,
            remove_stats_source_and_id: None,
        }
    );
    assert_eq!(
        evaluate_modifier_deactivate(true, false, 42),
        ModifierDeactivateOutcome {
            run_base_deactivate: true,
            remove_stats_source_and_id: None,
        }
    );
    assert_eq!(
        evaluate_modifier_deactivate(true, true, -1),
        ModifierDeactivateOutcome {
            run_base_deactivate: true,
            remove_stats_source_and_id: Some((MODIFIER_STAT_SOURCE_KIND, -1)),
        }
    );
    assert_eq!(
        evaluate_modifier_deactivate(true, true, 77),
        ModifierDeactivateOutcome {
            run_base_deactivate: true,
            remove_stats_source_and_id: Some((3, 77)),
        }
    );

    // Activate (+0x278)
    assert_eq!(
        evaluate_modifier_activate(true, true, true, true, true),
        ModifierActivateOutcome::AlreadyActiveOrMissingOwner {
            forwarded_b_reset: true,
        }
    );
    assert_eq!(
        evaluate_modifier_activate(false, false, false, true, true),
        ModifierActivateOutcome::AlreadyActiveOrMissingOwner {
            forwarded_b_reset: false,
        }
    );
    assert_eq!(
        evaluate_modifier_activate(false, true, true, false, false),
        ModifierActivateOutcome::NullStatContainerSparseDelegateCrash {
            forwarded_b_reset: true,
        }
    );
    assert_eq!(
        evaluate_modifier_activate(false, false, true, true, true),
        ModifierActivateOutcome::BoundDelegateAndUpdatedStats {
            forwarded_b_reset: false,
            set_stat_container_flag_b3: true,
            dispatch_update_stats: true,
        }
    );
    assert_eq!(
        evaluate_modifier_activate(false, true, true, true, false),
        ModifierActivateOutcome::BoundDelegateAndUpdatedStats {
            forwarded_b_reset: true,
            set_stat_container_flag_b3: false,
            dispatch_update_stats: true,
        }
    );

    // UpdateStats (+0x408)
    assert_eq!(
        evaluate_modifier_update_stats(false, true, 77, 4),
        ModifierUpdateStatsOutcome::InactiveNoOp
    );
    assert_eq!(
        evaluate_modifier_update_stats(true, false, 77, 4),
        ModifierUpdateStatsOutcome::MissingStatContainerNoOp
    );
    assert_eq!(
        evaluate_modifier_update_stats(true, true, 77, 0),
        ModifierUpdateStatsOutcome::ReplacedModifierStats {
            removed_source_and_id: (3, 77),
            added_source_and_id: (3, 77),
            converted_stat_entries: 0,
        }
    );
    assert_eq!(
        evaluate_modifier_update_stats(true, true, 77, 5),
        ModifierUpdateStatsOutcome::ReplacedModifierStats {
            removed_source_and_id: (3, 77),
            added_source_and_id: (3, 77),
            converted_stat_entries: 5,
        }
    );
}

#[test]
fn modifier_begin_play_and_end_play_rules_cover_authority_and_reason_gates() {
    // BeginPlay (+0x338)
    for role in [0, 1, 2, 4] {
        assert_eq!(
            evaluate_modifier_begin_play(role, false),
            ModifierBeginPlayOutcome::NonAuthorityBaseOnly
        );
    }
    assert_eq!(
        evaluate_modifier_begin_play(MODIFIER_AUTHORITY_NET_ROLE, false),
        ModifierBeginPlayOutcome::AuthorityInitialized {
            cache_modifier_state_data_140: true,
            call_modifier_applied: true,
            call_init_component: true,
        }
    );
    assert_eq!(
        evaluate_modifier_begin_play(MODIFIER_AUTHORITY_NET_ROLE, true),
        ModifierBeginPlayOutcome::AuthorityInitialized {
            cache_modifier_state_data_140: true,
            call_modifier_applied: false,
            call_init_component: true,
        }
    );

    // EndPlay (+0x340): non-zero reason skips all modifier teardown/notifications
    for reason in [1u8, 2, 3, 4] {
        assert_eq!(
            evaluate_modifier_end_play(&ModifierEndPlayInputs {
                end_play_reason: reason,
                owner_role: MODIFIER_AUTHORITY_NET_ROLE,
                has_world: true,
                world_has_owning_game_instance: true,
                activation_state_319: MODIFIER_FIX_ACTIVATION_STATE,
                has_attached_component_100: true,
            }),
            ModifierEndPlayOutcome::NonZeroReasonBaseAndClear3c0Only
        );
    }

    // EndPlay(0) on authority with GameInstance timer manager and +0x319 == 3
    assert_eq!(
        evaluate_modifier_end_play(&ModifierEndPlayInputs {
            end_play_reason: 0,
            owner_role: MODIFIER_AUTHORITY_NET_ROLE,
            has_world: true,
            world_has_owning_game_instance: true,
            activation_state_319: MODIFIER_FIX_ACTIVATION_STATE,
            has_attached_component_100: true,
        }),
        ModifierEndPlayOutcome::ReasonZeroTeardown {
            clear_field_3c0: true,
            call_modifier_removed: true,
            timer_clear_target: ModifierTimerClearTarget::GameInstanceTimerManager,
            dispatch_virtual_deactivate: true,
            call_fix_modifier_activation: true,
            destroy_attached_component_100: true,
            notify_owner_modifiable_interface_removed: true,
            broadcast_on_modifier_updated_removed: true,
        }
    );

    // EndPlay(0) on authority with World fallback timer manager and +0x319 != 3
    assert_eq!(
        evaluate_modifier_end_play(&ModifierEndPlayInputs {
            end_play_reason: 0,
            owner_role: MODIFIER_AUTHORITY_NET_ROLE,
            has_world: true,
            world_has_owning_game_instance: false,
            activation_state_319: 1,
            has_attached_component_100: false,
        }),
        ModifierEndPlayOutcome::ReasonZeroTeardown {
            clear_field_3c0: true,
            call_modifier_removed: true,
            timer_clear_target: ModifierTimerClearTarget::WorldTimerManager,
            dispatch_virtual_deactivate: true,
            call_fix_modifier_activation: false,
            destroy_attached_component_100: false,
            notify_owner_modifiable_interface_removed: true,
            broadcast_on_modifier_updated_removed: true,
        }
    );

    // EndPlay(0) on authority when World is null still runs Deactivate and notifications
    assert_eq!(
        evaluate_modifier_end_play(&ModifierEndPlayInputs {
            end_play_reason: 0,
            owner_role: MODIFIER_AUTHORITY_NET_ROLE,
            has_world: false,
            world_has_owning_game_instance: true,
            activation_state_319: 3,
            has_attached_component_100: false,
        }),
        ModifierEndPlayOutcome::ReasonZeroTeardown {
            clear_field_3c0: true,
            call_modifier_removed: true,
            timer_clear_target: ModifierTimerClearTarget::None,
            dispatch_virtual_deactivate: true,
            call_fix_modifier_activation: true,
            destroy_attached_component_100: false,
            notify_owner_modifiable_interface_removed: true,
            broadcast_on_modifier_updated_removed: true,
        }
    );

    // EndPlay(0) on non-authority skips removal/timers/Deactivate/FixModifierActivation
    // but still destroys +0x100 and fires OnModifiersUpdated + OnModifierUpdated
    assert_eq!(
        evaluate_modifier_end_play(&ModifierEndPlayInputs {
            end_play_reason: 0,
            owner_role: 1,
            has_world: true,
            world_has_owning_game_instance: true,
            activation_state_319: 3,
            has_attached_component_100: true,
        }),
        ModifierEndPlayOutcome::ReasonZeroTeardown {
            clear_field_3c0: true,
            call_modifier_removed: false,
            timer_clear_target: ModifierTimerClearTarget::None,
            dispatch_virtual_deactivate: false,
            call_fix_modifier_activation: false,
            destroy_attached_component_100: true,
            notify_owner_modifiable_interface_removed: true,
            broadcast_on_modifier_updated_removed: true,
        }
    );
}

#[test]
fn modifier_stat_scaling_and_state_data_rules_match_sse2_and_row_branches() {
    assert_eq!(MODIFIER_STAT_SCALE_CENTI_BITS, 0x3c23_d70a);
    // Sentinel 0xff skips scaling completely even when effectiveness != 0 or overflow would happen
    assert_eq!(
        scale_modifier_granted_stat_value(MODIFIER_INVALID_STAT_ENUM, 123, 500),
        (MODIFIER_INVALID_STAT_ENUM, 123)
    );
    assert_eq!(
        scale_modifier_granted_stat_value(MODIFIER_INVALID_STAT_ENUM, -77, -100),
        (MODIFIER_INVALID_STAT_ENUM, -77)
    );

    // Zero effectiveness (100% multiplier) preserves ordinary positive/negative/zero stats
    assert_eq!(scale_modifier_granted_stat_value(7, 0, 50), (7, 0));
    assert_eq!(scale_modifier_granted_stat_value(7, 25, 0), (7, 25));
    assert_eq!(scale_modifier_granted_stat_value(7, -25, 0), (7, -25));

    // Away-from-zero ceiling on magnitude (MSVC ceilf(|scaled|) * sgn):
    // .5 boundaries: +22.5 -> +23, -22.5 -> -23, +0.5 -> +1, -0.5 -> -1
    assert_eq!(scale_modifier_granted_stat_value(4, 15, 50), (4, 23));
    assert_eq!(scale_modifier_granted_stat_value(4, -15, 50), (4, -23));
    assert_eq!(scale_modifier_granted_stat_value(4, 1, -50), (4, 1));
    assert_eq!(scale_modifier_granted_stat_value(4, -1, -50), (4, -1));
    // Non-half fractions below .5 distinguish ceil(|scaled|) from nearest rounding:
    // +2.1 -> +3, -2.1 -> -3, +10.3 -> +11, -10.3 -> -11, +0.3 -> +1, -0.3 -> -1, +2.91 -> +3, -2.91 -> -3
    assert_eq!(scale_modifier_granted_stat_value(4, 2, 5), (4, 3));
    assert_eq!(scale_modifier_granted_stat_value(4, -2, 5), (4, -3));
    assert_eq!(scale_modifier_granted_stat_value(4, 10, 3), (4, 11));
    assert_eq!(scale_modifier_granted_stat_value(4, -10, 3), (4, -11));
    assert_eq!(scale_modifier_granted_stat_value(4, 1, -70), (4, 1));
    assert_eq!(scale_modifier_granted_stat_value(4, -1, -70), (4, -1));
    assert_eq!(scale_modifier_granted_stat_value(4, 3, -3), (4, 3));
    assert_eq!(scale_modifier_granted_stat_value(4, -3, -3), (4, -3));

    // -100% effectiveness zeroes out any finite raw stat; < -100% flips sign
    assert_eq!(scale_modifier_granted_stat_value(9, 40, -100), (9, 0));
    assert_eq!(scale_modifier_granted_stat_value(9, -40, -100), (9, 0));
    assert_eq!(scale_modifier_granted_stat_value(9, 10, -150), (9, -5));
    assert_eq!(scale_modifier_granted_stat_value(9, -10, -150), (9, 5));

    // Extreme values where 2 * abs >= 2^31 trigger CVTSS2SI integer indefinite (i32::MIN),
    // which SAR 1 + NEG + MULSS sgn + CVTTSS2SI maps to ±2^30 (±1_073_741_824)!
    assert_eq!(
        scale_modifier_granted_stat_value(1, i32::MAX, 1_000_000),
        (1, 1_073_741_824)
    );
    assert_eq!(
        scale_modifier_granted_stat_value(1, -i32::MAX, 1_000_000),
        (1, -1_073_741_824)
    );

    // GetModifierStateData row resolution
    assert_eq!(
        evaluate_modifier_get_state_data(true, true),
        ModifierStateDataSource::DefaultConstructedFromNoneRowName
    );
    assert_eq!(
        evaluate_modifier_get_state_data(true, false),
        ModifierStateDataSource::DefaultConstructedFromNoneRowName
    );
    assert_eq!(
        evaluate_modifier_get_state_data(false, true),
        ModifierStateDataSource::CopyConstructedFromTableRow
    );
    assert_eq!(
        evaluate_modifier_get_state_data(false, false),
        ModifierStateDataSource::CopyConstructedFromTemporaryDefault
    );
}

#[test]
fn modifier_init_component_applied_and_removed_rules_cover_all_branches() {
    // InitComponent_Implementation: empty soft path + high-frequency timer on GameInstance + initial tick
    assert_eq!(
        evaluate_modifier_init_component_impl(&ModifierInitComponentInputs {
            soft_scene_class_path_non_empty_1b0: false,
            resolved_valid_scene_component_class: false,
            enable_high_frequency_tick_300: true,
            world_field_5a0: 0x1234_5678,
            world_has_owning_game_instance: true,
            tick_immediately_301: true,
            check_tick_conditions_passed: true,
        }),
        ModifierInitComponentOutcome {
            scene_component_action: ModifierSceneComponentInitAction::SkippedEmptySoftPath,
            snapshot_world_5a0_into_b0: Some(0x1234_5678),
            high_frequency_timer_3c0: Some(ModifierLoopingTimerSetup {
                timer_manager: ModifierTimerClearTarget::GameInstanceTimerManager,
                rate_seconds: f32::from_bits(MODIFIER_HIGH_FREQUENCY_TICK_RATE_BITS),
                loop_timer: true,
                first_delay_seconds: MODIFIER_TIMER_DEFAULT_FIRST_DELAY,
            }),
            run_initial_modifier_tick_zero_dt: true,
        }
    );

    // InitComponent_Implementation: non-empty soft path with unresolved/non-USceneComponent class crashes at construction
    assert_eq!(
        evaluate_modifier_init_component_impl(&ModifierInitComponentInputs {
            soft_scene_class_path_non_empty_1b0: true,
            resolved_valid_scene_component_class: false,
            enable_high_frequency_tick_300: true,
            world_field_5a0: 99,
            world_has_owning_game_instance: false,
            tick_immediately_301: true,
            check_tick_conditions_passed: true,
        }),
        ModifierInitComponentOutcome {
            scene_component_action: ModifierSceneComponentInitAction::NullClassConstructCrash,
            snapshot_world_5a0_into_b0: None,
            high_frequency_timer_3c0: None,
            run_initial_modifier_tick_zero_dt: false,
        }
    );

    // InitComponent_Implementation: valid scene component + World timer fallback + failed CheckTickConditions
    assert_eq!(
        evaluate_modifier_init_component_impl(&ModifierInitComponentInputs {
            soft_scene_class_path_non_empty_1b0: true,
            resolved_valid_scene_component_class: true,
            enable_high_frequency_tick_300: true,
            world_field_5a0: 7,
            world_has_owning_game_instance: false,
            tick_immediately_301: true,
            check_tick_conditions_passed: false,
        }),
        ModifierInitComponentOutcome {
            scene_component_action:
                ModifierSceneComponentInitAction::ConstructRegisterAndAttachKeepRelative {
                    attachment_rules_packed: MODIFIER_SCENE_ATTACH_KEEP_RELATIVE_PACKED,
                },
            snapshot_world_5a0_into_b0: Some(7),
            high_frequency_timer_3c0: Some(ModifierLoopingTimerSetup {
                timer_manager: ModifierTimerClearTarget::WorldTimerManager,
                rate_seconds: f32::from_bits(0x3dcc_cccd),
                loop_timer: true,
                first_delay_seconds: -1.0,
            }),
            run_initial_modifier_tick_zero_dt: false,
        }
    );

    // ModifierApplied_Implementation
    assert_eq!(
        evaluate_modifier_applied_impl(false, 5, false, 2),
        ModifierAppliedOutcome::MissingOwnerFalse
    );
    assert_eq!(
        evaluate_modifier_applied_impl(true, 12, false, 0),
        ModifierAppliedOutcome::AppliedTrue {
            assigned_uid_f8: 12,
            set_flag_3b0: true,
            populate_instigator_controller_d0_from_owner: true,
            call_refresh_lifetime: true,
            append_granted_tags_to_owner: false,
            notify_owner_modifiable_interface_added: true,
            broadcast_on_modifier_updated_added: true,
        }
    );
    assert_eq!(
        evaluate_modifier_applied_impl(true, 13, true, 3),
        ModifierAppliedOutcome::AppliedTrue {
            assigned_uid_f8: 13,
            set_flag_3b0: true,
            populate_instigator_controller_d0_from_owner: false,
            call_refresh_lifetime: true,
            append_granted_tags_to_owner: true,
            notify_owner_modifiable_interface_added: true,
            broadcast_on_modifier_updated_added: true,
        }
    );

    // ModifierRemoved_Implementation
    assert_eq!(
        evaluate_modifier_removed_impl(false, &[101], &[]),
        ModifierRemovedOutcome::MissingOwnerFalse
    );
    assert_eq!(
        evaluate_modifier_removed_impl(true, &[], &[]),
        ModifierRemovedOutcome::EmptyGrantedTagsTrue
    );

    // Sibling subtraction:
    // - self is skipped
    // - sibling with empty gameplay_tags is skipped even if parent_tags has a match
    // - sibling gameplay_tags and parent_tags both protect valid tags from removal
    // - NAME_None (0) is never removed by sibling matching
    let siblings = [
        SiblingModifierGrantedTags {
            is_self: true,
            gameplay_tags: &[100, 200, 300],
            parent_tags: &[400],
        },
        SiblingModifierGrantedTags {
            is_self: false,
            gameplay_tags: &[],
            parent_tags: &[300],
        },
        SiblingModifierGrantedTags {
            is_self: false,
            gameplay_tags: &[100],
            parent_tags: &[200, 0],
        },
    ];
    assert_eq!(
        evaluate_modifier_removed_impl(true, &[0, 100, 200, 300], &siblings),
        ModifierRemovedOutcome::UpdatedOwnerTagsTrue {
            siblings_inspected: 2,
            remaining_tags_to_remove: vec![0, 300],
        }
    );

    // Early break once removal_set becomes empty
    let full_cover_siblings = [
        SiblingModifierGrantedTags {
            is_self: false,
            gameplay_tags: &[10],
            parent_tags: &[20],
        },
        SiblingModifierGrantedTags {
            is_self: false,
            gameplay_tags: &[99],
            parent_tags: &[],
        },
    ];
    assert_eq!(
        evaluate_modifier_removed_impl(true, &[10, 20], &full_cover_siblings),
        ModifierRemovedOutcome::UpdatedOwnerTagsTrue {
            siblings_inspected: 1,
            remaining_tags_to_remove: Vec::new(),
        }
    );
}

#[test]
fn modifier_on_stat_container_updated_and_fix_activation_match_native_gates() {
    // OnStatContainerUpdated: NaN or equal multiplier skips lifetime block; authority broadcasts +0x130
    let nan_out =
        evaluate_modifier_on_stat_container_updated(&ModifierOnStatContainerUpdatedInputs {
            old_lifetime_multiplier_10c: 1.0,
            new_lifetime_multiplier: f32::NAN,
            has_active_auto_destroy_timer_3a8: true,
            world_has_owning_game_instance: true,
            base_lifetime_108: 60.0,
            remaining_time_110: 30.0,
            owner_e0_has_stat_container: true,
            effectiveness_stat_values: &[15, 10],
            base_effectiveness_114: 5,
            old_effectiveness_128: 0,
            owner_role: MODIFIER_AUTHORITY_NET_ROLE,
        });
    assert_eq!(
        nan_out,
        ModifierOnStatContainerUpdatedOutcome {
            updated_lifetime_multiplier_10c: None,
            cleared_auto_destroy_timer_3a8: None,
            rescaled_remaining_time_110: None,
            restarted_auto_destroy_timer_3a8: None,
            updated_effectiveness_128: Some(30),
            broadcast_effectiveness_delegate_130: true,
            dispatch_update_stats: true,
        }
    );

    // OnStatContainerUpdated: multiplier 1.0 -> 2.0 rescales remaining 30/60 -> 60 and restarts AutoDestroy;
    // non-authority role skips +0x130 broadcast while still dispatching UpdateStats
    let rescale_out =
        evaluate_modifier_on_stat_container_updated(&ModifierOnStatContainerUpdatedInputs {
            old_lifetime_multiplier_10c: 1.0,
            new_lifetime_multiplier: 2.0,
            has_active_auto_destroy_timer_3a8: true,
            world_has_owning_game_instance: false,
            base_lifetime_108: 60.0,
            remaining_time_110: 30.0,
            owner_e0_has_stat_container: true,
            effectiveness_stat_values: &[-20],
            base_effectiveness_114: 0,
            old_effectiveness_128: -10,
            owner_role: 1,
        });
    assert_eq!(
        rescale_out,
        ModifierOnStatContainerUpdatedOutcome {
            updated_lifetime_multiplier_10c: Some(2.0),
            cleared_auto_destroy_timer_3a8: Some(ModifierTimerClearTarget::WorldTimerManager),
            rescaled_remaining_time_110: Some(60.0),
            restarted_auto_destroy_timer_3a8: Some(ModifierLoopingTimerSetup {
                timer_manager: ModifierTimerClearTarget::WorldTimerManager,
                rate_seconds: 60.0,
                loop_timer: true,
                first_delay_seconds: -1.0,
            }),
            updated_effectiveness_128: Some(-20),
            broadcast_effectiveness_delegate_130: false,
            dispatch_update_stats: true,
        }
    );

    // OnStatContainerUpdated: remaining_time > base * old_mult skips SetRemainingTime but still restarts AutoDestroy
    let skip_rescale =
        evaluate_modifier_on_stat_container_updated(&ModifierOnStatContainerUpdatedInputs {
            old_lifetime_multiplier_10c: 1.0,
            new_lifetime_multiplier: 1.5,
            has_active_auto_destroy_timer_3a8: false,
            world_has_owning_game_instance: true,
            base_lifetime_108: 60.0,
            remaining_time_110: 75.0,
            owner_e0_has_stat_container: false,
            effectiveness_stat_values: &[999],
            base_effectiveness_114: 10,
            old_effectiveness_128: 10,
            owner_role: MODIFIER_AUTHORITY_NET_ROLE,
        });
    assert_eq!(
        skip_rescale,
        ModifierOnStatContainerUpdatedOutcome {
            updated_lifetime_multiplier_10c: Some(1.5),
            cleared_auto_destroy_timer_3a8: None,
            rescaled_remaining_time_110: None,
            restarted_auto_destroy_timer_3a8: Some(ModifierLoopingTimerSetup {
                timer_manager: ModifierTimerClearTarget::GameInstanceTimerManager,
                rate_seconds: 75.0,
                loop_timer: true,
                first_delay_seconds: -1.0,
            }),
            updated_effectiveness_128: None,
            broadcast_effectiveness_delegate_130: false,
            dispatch_update_stats: false,
        }
    );

    // FixModifierActivation: guard failures return empty actions
    let base_guards = FixModifierActivationGuards {
        actor_present: true,
        actor_implements_modifiable_interface: true,
        target_row_handle_valid: true,
        actor_has_world: true,
        target_row_name: 42,
    };
    let candidates = [
        // Different row -> ignored
        FixModifierActivationCandidate {
            component_token: 0,
            row_name: 99,
            b_is_being_destroyed: false,
            b_auto_activate: true,
            uid_f8: 1,
        },
        // Being destroyed -> ignored
        FixModifierActivationCandidate {
            component_token: 1,
            row_name: 42,
            b_is_being_destroyed: true,
            b_auto_activate: true,
            uid_f8: 2,
        },
        // Lowest UID (3) but !bAutoActivate -> Deactivate
        FixModifierActivationCandidate {
            component_token: 2,
            row_name: 42,
            b_is_being_destroyed: false,
            b_auto_activate: false,
            uid_f8: 3,
        },
        // UID 10 visited first
        FixModifierActivationCandidate {
            component_token: 3,
            row_name: 42,
            b_is_being_destroyed: false,
            b_auto_activate: true,
            uid_f8: 10,
        },
        // Equal UID 10 visited second: LowerBoundBy inserts BEFORE token 3, so token 4 wins Activate(false)!
        FixModifierActivationCandidate {
            component_token: 4,
            row_name: 42,
            b_is_being_destroyed: false,
            b_auto_activate: true,
            uid_f8: 10,
        },
        // Higher UID 20 -> Deactivate
        FixModifierActivationCandidate {
            component_token: 5,
            row_name: 42,
            b_is_being_destroyed: false,
            b_auto_activate: true,
            uid_f8: 20,
        },
    ];
    for invalid_guards in [
        FixModifierActivationGuards {
            actor_present: false,
            ..base_guards
        },
        FixModifierActivationGuards {
            actor_implements_modifiable_interface: false,
            ..base_guards
        },
        FixModifierActivationGuards {
            target_row_handle_valid: false,
            ..base_guards
        },
        FixModifierActivationGuards {
            actor_has_world: false,
            ..base_guards
        },
    ] {
        assert!(select_fix_modifier_activation_actions(&invalid_guards, &candidates).is_empty());
    }

    assert_eq!(
        select_fix_modifier_activation_actions(&base_guards, &candidates),
        vec![
            FixModifierActivationAction::Deactivate { component_token: 2 },
            FixModifierActivationAction::ActivateResetFalse { component_token: 4 },
            FixModifierActivationAction::Deactivate { component_token: 3 },
            FixModifierActivationAction::Deactivate { component_token: 5 },
        ]
    );
}

#[test]
fn exhaustive_modifier_lifecycle_invariants() {
    for b_was_active in [false, true] {
        for b_reset in [false, true] {
            for owner_present in [false, true] {
                for has_stat in [false, true] {
                    for added in [false, true] {
                        let act = evaluate_modifier_activate(
                            b_was_active,
                            b_reset,
                            owner_present,
                            has_stat,
                            added,
                        );
                        let deact = evaluate_modifier_deactivate(owner_present, has_stat, 99);
                        assert!(deact.run_base_deactivate);
                        assert_eq!(
                            deact.remove_stats_source_and_id.is_some(),
                            owner_present && has_stat
                        );
                        match act {
                            ModifierActivateOutcome::AlreadyActiveOrMissingOwner {
                                forwarded_b_reset,
                            } => {
                                assert_eq!(forwarded_b_reset, b_reset);
                                assert!(b_was_active || !owner_present);
                            }
                            ModifierActivateOutcome::NullStatContainerSparseDelegateCrash {
                                forwarded_b_reset,
                            } => {
                                assert_eq!(forwarded_b_reset, b_reset);
                                assert!(!b_was_active && owner_present && !has_stat);
                            }
                            ModifierActivateOutcome::BoundDelegateAndUpdatedStats {
                                forwarded_b_reset,
                                set_stat_container_flag_b3,
                                dispatch_update_stats,
                            } => {
                                assert_eq!(forwarded_b_reset, b_reset);
                                assert_eq!(set_stat_container_flag_b3, added);
                                assert!(dispatch_update_stats);
                                assert!(!b_was_active && owner_present && has_stat);
                            }
                        }
                    }
                }
            }
        }
    }

    for reason in [0u8, 1, 2] {
        for role in [0, 1, 2, 3] {
            for has_world in [false, true] {
                for has_gi in [false, true] {
                    for state_319 in [0u8, 2, 3, 4] {
                        for has_100 in [false, true] {
                            let out = evaluate_modifier_end_play(&ModifierEndPlayInputs {
                                end_play_reason: reason,
                                owner_role: role,
                                has_world,
                                world_has_owning_game_instance: has_gi,
                                activation_state_319: state_319,
                                has_attached_component_100: has_100,
                            });
                            if reason != 0 {
                                assert_eq!(
                                    out,
                                    ModifierEndPlayOutcome::NonZeroReasonBaseAndClear3c0Only
                                );
                            } else if let ModifierEndPlayOutcome::ReasonZeroTeardown {
                                clear_field_3c0,
                                call_modifier_removed,
                                timer_clear_target,
                                dispatch_virtual_deactivate,
                                call_fix_modifier_activation,
                                destroy_attached_component_100,
                                notify_owner_modifiable_interface_removed,
                                broadcast_on_modifier_updated_removed,
                            } = out
                            {
                                assert!(clear_field_3c0);
                                assert_eq!(call_modifier_removed, role == 3);
                                assert_eq!(dispatch_virtual_deactivate, role == 3);
                                assert_eq!(
                                    call_fix_modifier_activation,
                                    role == 3 && state_319 == 3
                                );
                                assert_eq!(
                                    timer_clear_target != ModifierTimerClearTarget::None,
                                    role == 3 && has_world
                                );
                                assert_eq!(destroy_attached_component_100, has_100);
                                assert!(notify_owner_modifiable_interface_removed);
                                assert!(broadcast_on_modifier_updated_removed);
                            } else {
                                panic!("expected ReasonZeroTeardown when end_play_reason == 0");
                            }
                        }
                    }
                }
            }
        }
    }
}

fn removal_candidate(
    uid_f8: i32,
    flags_8b: u8,
    opaque_comparison_result: bool,
) -> RemoveModifierStateCandidate {
    RemoveModifierStateCandidate {
        flags_8b,
        uid_f8,
        opaque_comparison_result,
    }
}

fn assert_local_removal_request(
    outcome: &RemoveModifierStateSelectionOutcome,
    selected_input_index: Option<usize>,
) {
    // An Option pins the cardinality to at most one modeled request. The flag
    // describes the caller after return, with no claim about destruction.
    let expected_request =
        selected_input_index.map(|input_index| RemoveModifierStateVirtualCallRequest {
            input_index,
            vftable_byte_offset: 0x3c0,
            edx: 0,
        });
    assert_eq!(outcome.virtual_call_request, expected_request);
    assert_eq!(
        outcome.local_success_after_request_returns,
        selected_input_index.is_some()
    );
    match outcome.selection {
        Some(RemoveModifierStateSelection::ExplicitUid { input_index })
        | Some(RemoveModifierStateSelection::LastComparisonTrue { input_index }) => {
            assert_eq!(Some(input_index), selected_input_index);
        }
        None => assert_eq!(selected_input_index, None),
    }
}

#[test]
fn local_removal_empty_or_all_flagged_has_no_selection_or_request() {
    let all_flagged = [
        removal_candidate(7, 0x02, true),
        removal_candidate(7, 0xff, true),
        removal_candidate(-1, 0x82, false),
    ];
    for candidates in [&[][..], &all_flagged[..]] {
        let outcome = select_remove_modifier_state_local(7, candidates);
        assert!(outcome.filtered_input_indices.is_empty());
        assert!(outcome.compared_input_indices.is_empty());
        assert_eq!(outcome.selection, None);
        assert_local_removal_request(&outcome, None);
    }
}

#[test]
fn local_removal_first_uid_match_overrides_comparison_and_stops_the_scan() {
    let candidates = [
        removal_candidate(90, 0, true),
        removal_candidate(7, 0, false),
        removal_candidate(7, 0, true),
        removal_candidate(8, 0, true),
    ];
    let outcome = select_remove_modifier_state_local(7, &candidates);
    // The entire filtering pass precedes selection, even though selection stops
    // at index 1. Neither the selected candidate nor later inputs are compared.
    assert_eq!(outcome.filtered_input_indices, vec![0, 1, 2, 3]);
    assert_eq!(outcome.compared_input_indices, vec![0]);
    assert_eq!(
        outcome.selection,
        Some(RemoveModifierStateSelection::ExplicitUid { input_index: 1 })
    );
    assert_local_removal_request(&outcome, Some(1));
}

#[test]
fn local_removal_duplicate_uids_choose_first_retained_without_comparison() {
    let candidates = [
        removal_candidate(7, 0x02, true),
        removal_candidate(7, 0x01, false),
        removal_candidate(7, 0x80, true),
    ];
    let outcome = select_remove_modifier_state_local(7, &candidates);
    assert_eq!(outcome.filtered_input_indices, vec![1, 2]);
    assert!(outcome.compared_input_indices.is_empty());
    assert_eq!(
        outcome.selection,
        Some(RemoveModifierStateSelection::ExplicitUid { input_index: 1 })
    );
    assert_local_removal_request(&outcome, Some(1));
}

#[test]
fn local_removal_uid_mismatch_uses_last_true_in_enumeration_order() {
    let candidates = [
        removal_candidate(90, 0, true),
        removal_candidate(120, 0, false),
        removal_candidate(-80, 0, true),
        removal_candidate(40, 0, true),
        removal_candidate(i32::MIN, 0, false),
    ];
    let outcome = select_remove_modifier_state_local(7, &candidates);
    assert_eq!(outcome.filtered_input_indices, vec![0, 1, 2, 3, 4]);
    assert_eq!(outcome.compared_input_indices, vec![0, 1, 2, 3, 4]);
    // UID 40 is neither the smallest nor largest comparison-true UID.
    assert_eq!(
        outcome.selection,
        Some(RemoveModifierStateSelection::LastComparisonTrue { input_index: 3 })
    );
    assert_local_removal_request(&outcome, Some(3));
}

#[test]
fn local_removal_minus_one_suppresses_uid_only_even_for_component_minus_one() {
    let candidates = [
        removal_candidate(-1, 0, true),
        removal_candidate(4, 0, false),
        removal_candidate(-1, 0, true),
    ];
    let outcome = select_remove_modifier_state_local(-1, &candidates);
    assert_eq!(outcome.filtered_input_indices, vec![0, 1, 2]);
    assert_eq!(outcome.compared_input_indices, vec![0, 1, 2]);
    assert_eq!(
        outcome.selection,
        Some(RemoveModifierStateSelection::LastComparisonTrue { input_index: 2 })
    );
    assert_local_removal_request(&outcome, Some(2));

    let no_match = select_remove_modifier_state_local(-1, &[removal_candidate(-1, 0, false)]);
    assert_eq!(no_match.filtered_input_indices, vec![0]);
    assert_eq!(no_match.compared_input_indices, vec![0]);
    assert_eq!(no_match.selection, None);
    assert_local_removal_request(&no_match, None);
}

#[test]
fn local_removal_filters_flags_before_uid_and_comparison_selection() {
    let candidates = [
        removal_candidate(7, 0x02, true),
        removal_candidate(8, 0x01, true),
        removal_candidate(9, 0x00, false),
        removal_candidate(10, 0xfe, true),
    ];
    let outcome = select_remove_modifier_state_local(7, &candidates);
    assert_eq!(outcome.filtered_input_indices, vec![1, 2]);
    assert_eq!(outcome.compared_input_indices, vec![1, 2]);
    assert_eq!(
        outcome.selection,
        Some(RemoveModifierStateSelection::LastComparisonTrue { input_index: 1 })
    );
    assert_local_removal_request(&outcome, Some(1));

    // Only mask 0x02 matters; unrelated raw bits must not exclude a candidate.
    for flags in u8::MIN..=u8::MAX {
        let outcome = select_remove_modifier_state_local(7, &[removal_candidate(7, flags, false)]);
        if flags & 0x02 == 0 {
            assert_eq!(outcome.filtered_input_indices, vec![0]);
            assert_eq!(
                outcome.selection,
                Some(RemoveModifierStateSelection::ExplicitUid { input_index: 0 })
            );
            assert_local_removal_request(&outcome, Some(0));
        } else {
            assert!(outcome.filtered_input_indices.is_empty());
            assert_eq!(outcome.selection, None);
            assert_local_removal_request(&outcome, None);
        }
        assert!(outcome.compared_input_indices.is_empty());
    }
}

#[test]
fn local_removal_no_uid_or_comparison_match_has_no_request_or_success_flag() {
    let candidates = [
        removal_candidate(1, 0, false),
        removal_candidate(2, 0x80, false),
        removal_candidate(3, 0x02, true),
    ];
    let outcome = select_remove_modifier_state_local(7, &candidates);
    assert_eq!(outcome.filtered_input_indices, vec![0, 1]);
    assert_eq!(outcome.compared_input_indices, vec![0, 1]);
    assert_eq!(outcome.selection, None);
    assert_local_removal_request(&outcome, None);
}

#[test]
fn local_removal_preserves_signed_uid_boundaries_and_only_minus_one_is_sentinel() {
    for requested_uid in [i32::MIN, i32::MIN + 1, -2, 0, 1, i32::MAX - 1, i32::MAX] {
        // The boundary UID must override an earlier opaque comparison result.
        let candidates = [
            removal_candidate(-1, 0, true),
            removal_candidate(requested_uid, 0, false),
            removal_candidate(requested_uid, 0, true),
        ];
        let outcome = select_remove_modifier_state_local(requested_uid, &candidates);
        assert_eq!(outcome.filtered_input_indices, vec![0, 1, 2]);
        assert_eq!(outcome.compared_input_indices, vec![0]);
        assert_eq!(
            outcome.selection,
            Some(RemoveModifierStateSelection::ExplicitUid { input_index: 1 })
        );
        assert_local_removal_request(&outcome, Some(1));
    }
}

// Entirely authored state and operation scripts. Neither opaque type implements
// Copy, Clone or equality; the comparator must not require those capabilities.
struct AuthoredComparatorOperand(u8);
struct AuthoredComparatorResultToken(usize);

#[derive(Debug, PartialEq, Eq)]
enum ComparatorLocalEvent {
    Normalize {
        operand: u8,
        scratch_slot: ModifierComparatorScratchSlot,
    },
    Read {
        token: usize,
        word: u64,
    },
}

struct AuthoredComparatorOperations {
    result_indices: [usize; 2],
    words: [u64; 2],
    normalizations: usize,
    reads: usize,
    second_normalization_write: Option<(usize, u64)>,
    first_read_write: Option<(usize, u64)>,
    events: Vec<ComparatorLocalEvent>,
}

impl AuthoredComparatorOperations {
    fn new(result_indices: [usize; 2], words: [u64; 2]) -> Self {
        Self {
            result_indices,
            words,
            normalizations: 0,
            reads: 0,
            second_normalization_write: None,
            first_read_write: None,
            events: Vec::new(),
        }
    }

    fn assert_order(&self, left: u8, right: u8, left_word: u64, right_word: u64) {
        assert_eq!(self.normalizations, 2);
        assert_eq!(self.reads, 2);
        assert_eq!(
            self.events,
            vec![
                ComparatorLocalEvent::Normalize {
                    operand: right,
                    scratch_slot: ModifierComparatorScratchSlot::FirstRight,
                },
                ComparatorLocalEvent::Normalize {
                    operand: left,
                    scratch_slot: ModifierComparatorScratchSlot::SecondLeft,
                },
                ComparatorLocalEvent::Read {
                    token: self.result_indices[1],
                    word: left_word,
                },
                ComparatorLocalEvent::Read {
                    token: self.result_indices[0],
                    word: right_word,
                },
            ]
        );
    }
}

impl ModifierComparatorOperations for AuthoredComparatorOperations {
    type Operand = AuthoredComparatorOperand;
    type ResultToken = AuthoredComparatorResultToken;

    fn normalize(
        &mut self,
        operand: &Self::Operand,
        scratch_slot: ModifierComparatorScratchSlot,
    ) -> Self::ResultToken {
        self.events.push(ComparatorLocalEvent::Normalize {
            operand: operand.0,
            scratch_slot,
        });
        let token = AuthoredComparatorResultToken(self.result_indices[self.normalizations]);
        self.normalizations += 1;
        if self.normalizations == 2
            && let Some((index, word)) = self.second_normalization_write
        {
            self.words[index] = word;
        }
        token
    }

    fn read_u64(&mut self, result_token: &Self::ResultToken) -> u64 {
        assert_eq!(self.normalizations, 2, "both calls must precede any read");
        let word = self.words[result_token.0];
        self.events.push(ComparatorLocalEvent::Read {
            token: result_token.0,
            word,
        });
        self.reads += 1;
        if self.reads == 1
            && let Some((index, word)) = self.first_read_write
        {
            self.words[index] = word;
        }
        word
    }
}

#[test]
fn local_comparator_orders_requests_with_distinct_scratch_then_left_and_right_reads() {
    let left = AuthoredComparatorOperand(17);
    let right = AuthoredComparatorOperand(29);
    let mut operations = AuthoredComparatorOperations::new([0, 1], [7, 8]);

    assert_ne!(
        ModifierComparatorScratchSlot::FirstRight,
        ModifierComparatorScratchSlot::SecondLeft
    );
    assert!(!evaluate_modifier_comparator_local(
        &mut operations,
        &left,
        &right
    ));
    operations.assert_order(17, 29, 8, 7);
}

#[test]
fn local_comparator_compares_all_u64_bits_and_allows_different_tokens_with_equal_words() {
    let left = AuthoredComparatorOperand(17);
    let right = AuthoredComparatorOperand(29);
    let boundary_words = [
        0,
        1,
        u64::from(u32::MAX),
        1_u64 << 63,
        (1_u64 << 63) | 1,
        u64::MAX,
    ];

    for left_word in boundary_words {
        for right_word in boundary_words {
            // Different token indices include equal-word cases on the diagonal.
            let mut operations = AuthoredComparatorOperations::new([0, 1], [right_word, left_word]);
            assert_eq!(
                evaluate_modifier_comparator_local(&mut operations, &left, &right),
                left_word == right_word,
                "left={left_word:#018x}, right={right_word:#018x}"
            );
            operations.assert_order(17, 29, left_word, right_word);
        }
    }
}

#[test]
fn local_comparator_same_operand_and_equal_tokens_still_make_both_calls_and_reads() {
    let operand = AuthoredComparatorOperand(17);
    let mut operations = AuthoredComparatorOperations::new([0, 0], [u64::MAX, 0]);

    assert!(evaluate_modifier_comparator_local(
        &mut operations,
        &operand,
        &operand
    ));
    operations.assert_order(17, 17, u64::MAX, u64::MAX);
}

#[test]
fn local_comparator_second_normalization_can_mutate_the_first_result_referent() {
    let left = AuthoredComparatorOperand(17);
    let right = AuthoredComparatorOperand(29);

    for updated_right_word in [9, 10] {
        let mut operations = AuthoredComparatorOperations::new([0, 1], [7, 9]);
        operations.second_normalization_write = Some((0, updated_right_word));

        assert_eq!(
            evaluate_modifier_comparator_local(&mut operations, &left, &right),
            updated_right_word == 9
        );
        // The right result is a retained token, not a word snapshot taken at
        // the first return. The second call changes what the later read sees.
        operations.assert_order(17, 29, 9, updated_right_word);
    }
}

#[test]
fn local_comparator_aliased_tokens_read_updated_shared_state_twice_after_calls() {
    let left = AuthoredComparatorOperand(17);
    let right = AuthoredComparatorOperand(29);
    let mut operations = AuthoredComparatorOperations::new([0, 0], [0, 0]);
    operations.second_normalization_write = Some((0, u64::MAX));
    // Authored read effects demonstrate that neither equal tokens nor a prior
    // read permits skipping the second callback or inventing a pure snapshot.
    operations.first_read_write = Some((0, 0));

    assert!(!evaluate_modifier_comparator_local(
        &mut operations,
        &left,
        &right
    ));
    operations.assert_order(17, 29, u64::MAX, 0);
}
