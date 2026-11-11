# L-809a test SFTP server: Dropbear. Dropbear has no SFTP subsystem of its own; as usual it
# runs OpenSSH's sftp-server binary (Alpine's dropbear is built to look for it at
# /usr/lib/ssh/sftp-server, where openssh-sftp-server installs it).
FROM docker.io/library/alpine:3.24.3
RUN apk add --no-cache dropbear openssh-sftp-server \
 && mkdir -p /home && adduser -D -s /bin/sh odt && sed -i 's/^odt:!/odt:*/' /etc/shadow
