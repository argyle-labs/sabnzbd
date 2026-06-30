# TODO: base image + build for sabnzbd. Mirror jellyfin/Dockerfile conventions.
FROM debian:12-slim
LABEL org.opencontainers.image.source="https://github.com/argyle-labs/sabnzbd"
EXPOSE 8080
