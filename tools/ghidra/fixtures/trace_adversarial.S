/* Independently authored x86-64 input for TraceBoundedFunctions integration.
 * Compile for inspection only; the runner never executes this ELF.
 */
.text
.global authored_good_gate
.type authored_good_gate,@function
authored_good_gate:
 test %edi,%edi
 je authored_good_gate_disabled
 cmp $-1,%esi
 je authored_good_gate_sentinel
 mov %esi,%eax
 ret
.global authored_good_gate_sentinel
authored_good_gate_sentinel:
 mov $100,%eax
 ret
.global authored_good_gate_disabled
authored_good_gate_disabled:
 mov $-7,%eax
 ret
.global authored_good_gate_end
authored_good_gate_end:
.size authored_good_gate,.-authored_good_gate

.global authored_xbegin_gate
.type authored_xbegin_gate,@function
authored_xbegin_gate:
 /* XBEGIN's abort destination is exactly outside the selected function. */
 .byte 0xc7,0xf8,0x01,0,0,0,0xc3
.global authored_xbegin_gate_end
authored_xbegin_gate_end:
.size authored_xbegin_gate,.-authored_xbegin_gate

.global authored_override_gate
.type authored_override_gate,@function
authored_override_gate:
 test %edi,%edi
.global authored_override_gate_jump
authored_override_gate_jump:
 je 1f
.global authored_override_gate_fallthrough
authored_override_gate_fallthrough:
 mov $7,%eax
 ret
1:
 mov $9,%eax
 ret
.global authored_override_gate_end
authored_override_gate_end:
.size authored_override_gate,.-authored_override_gate

.global authored_inline_caller
.type authored_inline_caller,@function
authored_inline_caller:
 call authored_inline_callee
 ret
.global authored_inline_caller_end
authored_inline_caller_end:
.size authored_inline_caller,.-authored_inline_caller
.global authored_inline_callee
.type authored_inline_callee,@function
authored_inline_callee:
 mov $7,%eax
 ret
.global authored_inline_callee_end
authored_inline_callee_end:
.size authored_inline_callee,.-authored_inline_callee

.global authored_mode_gate
.type authored_mode_gate,@function
authored_mode_gate:
 .byte 0x48,0x89,0xe5,0xc3
.global authored_mode_gate_end
authored_mode_gate_end:
.size authored_mode_gate,.-authored_mode_gate

.global authored_many_instructions
.type authored_many_instructions,@function
authored_many_instructions:
 .rept 5000
 nop
 .endr
 ret
.global authored_many_instructions_end
authored_many_instructions_end:
.size authored_many_instructions,.-authored_many_instructions

.global authored_straddle
.type authored_straddle,@function
authored_straddle:
 movabs $0x1122334455667788,%rax
 ret
.global authored_straddle_end
authored_straddle_end:
.size authored_straddle,.-authored_straddle

.global authored_many_pcode
.type authored_many_pcode,@function
authored_many_pcode:
 /* Flag-producing ADD exceeds the raw-pcode budget before 4096 instructions. */
 .rept 3000
 add $1,%eax
 .endr
 ret
.global authored_many_pcode_end
authored_many_pcode_end:
.size authored_many_pcode,.-authored_many_pcode
.global authored_undefined_override
.type authored_undefined_override,@function
authored_undefined_override:
 ret
.global authored_undefined_override_end
authored_undefined_override_end:
.size authored_undefined_override,.-authored_undefined_override
 /* Keep the final exclusive-end symbol inside the imported .text block. */
 nop
.section .note.GNU-stack,"",@progbits
