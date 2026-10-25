use icarus_bevy::experimental::modifier_lifecycle::{
    MODIFIER_AUTHORITY_NET_ROLE, MODIFIER_FIX_ACTIVATION_STATE, MODIFIER_STAT_SOURCE_KIND,
    ModifierActivateOutcome, ModifierBeginPlayOutcome, ModifierDeactivateOutcome,
    ModifierEndPlayInputs, ModifierEndPlayOutcome, ModifierPrimaryVftableSlot,
    ModifierTimerClearTarget, ModifierUpdateStatsOutcome, evaluate_modifier_activate,
    evaluate_modifier_begin_play, evaluate_modifier_deactivate, evaluate_modifier_end_play,
    evaluate_modifier_update_stats, resolve_modifier_primary_vftable_slot,
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
        (0x408, ModifierPrimaryVftableSlot::ModifierUpdateStats),
        (
            0x410,
            ModifierPrimaryVftableSlot::ModifierAppliedImplementation,
        ),
        (
            0x418,
            ModifierPrimaryVftableSlot::ModifierRemovedImplementation,
        ),
    ];
    for (offset, slot) in expected {
        assert_eq!(resolve_modifier_primary_vftable_slot(offset), Some(slot));
    }
    assert_eq!(resolve_modifier_primary_vftable_slot(0x000), None);
    assert_eq!(resolve_modifier_primary_vftable_slot(0x400), None);
    assert_eq!(resolve_modifier_primary_vftable_slot(0x420), None);
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
