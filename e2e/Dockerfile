FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y \
    openssh-server \
    systemd \
    systemd-sysv \
    nginx \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Configure sshd
RUN mkdir -p /run/sshd && \
    sed -i 's/#PermitRootLogin.*/PermitRootLogin prohibit-password/' /etc/ssh/sshd_config && \
    sed -i 's/#PubkeyAuthentication.*/PubkeyAuthentication yes/' /etc/ssh/sshd_config

# Create .ssh directory for root
RUN mkdir -p /root/.ssh && chmod 700 /root/.ssh

# The authorized_keys file will be mounted at runtime

EXPOSE 22

# Start sshd in foreground
CMD ["/usr/sbin/sshd", "-D", "-e"]
