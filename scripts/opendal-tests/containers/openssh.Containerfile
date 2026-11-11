# L-809a test SFTP server: OpenSSH sshd with its sftp-server subsystem.
FROM docker.io/library/alpine:3.24.3
RUN apk add --no-cache openssh-server openssh-sftp-server \
 && mkdir -p /home && adduser -D -s /bin/sh odt && sed -i 's/^odt:!/odt:*/' /etc/shadow \
 && mkdir -p /etc/ssh/keys /var/empty
