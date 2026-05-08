/* WilOS kernel link script (i386, multiboot 1).
 *
 * The kernel is linked at 1 MiB which is the standard load address used
 * by GRUB for multiboot kernels. The multiboot header sits at the very
 * start of the binary so GRUB can find it within the first 8 KiB.
 */

ENTRY(_start)

SECTIONS
{
    . = 1M;

    .multiboot ALIGN(4K) : { *(.multiboot) }

    .text ALIGN(4K) : {
        *(.text*)
    }

    .rodata ALIGN(4K) : {
        *(.rodata*)
    }

    .data ALIGN(4K) : {
        *(.data*)
    }

    .bss ALIGN(4K) : {
        __bss_start = .;
        *(COMMON)
        *(.bss*)
        __bss_end = .;
    }

    __kernel_end = .;
}
