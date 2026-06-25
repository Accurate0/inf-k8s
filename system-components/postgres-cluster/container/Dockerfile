FROM ghcr.io/cloudnative-pg/postgresql:18.4-standard-trixie

USER root
RUN set -xe ;\
    apt update && apt install build-essential pgxnclient -y;

# TimescaleDB
RUN apt-get install -y --no-install-recommends gnupg postgresql-common apt-transport-https lsb-release wget
RUN echo "deb https://packagecloud.io/timescale/timescaledb/debian/ $(lsb_release -c -s) main" \
    | tee /etc/apt/sources.list.d/timescaledb.list

RUN wget --quiet -O - https://packagecloud.io/timescale/timescaledb/gpgkey \
    | gpg --dearmor -o /etc/apt/trusted.gpg.d/timescaledb.gpg

RUN set -xe ;\
    apt update;\
    apt-get install -y --no-install-recommends timescaledb-2-postgresql-18

RUN set -xe ;\
    apt-get install -y --no-install-recommends postgresql-18-postgis-3

RUN pgxn install pgmq==1.11.1

RUN usermod -u 26 postgres

USER 26
