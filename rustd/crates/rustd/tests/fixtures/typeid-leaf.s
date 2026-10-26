.text
.type typeid_leaf,@function
typeid_leaf:
 mov %rdi,%rax
 mov typeid_value(%rip),%rcx
 mov %rcx,(%rdi)
 mov typeid_value+8(%rip),%rcx
 mov %rcx,8(%rdi)
 ret
.size typeid_leaf,.-typeid_leaf
.section .rodata
.balign 8
typeid_value:
 .quad 0x0123456789abcdef
 .quad 0xfedcba9876543210
