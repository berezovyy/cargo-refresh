# Runtime qualification only; release builds use Zig on the host.
FROM rockylinux:8
RUN dnf -y install python3 gcc binutils make which && dnf clean all
