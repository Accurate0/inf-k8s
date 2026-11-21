FROM ghcr.io/cloudnative-pg/postgresql:18.6-standard-trixie

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
    apt-get install -y --no-install-recommends \
        timescaledb-2-postgresql-18=2.31.0~debian13-1806 \
        timescaledb-2-loader-postgresql-18=2.31.0~debian13-1806

RUN set -xe ;\
    apt-get install -y --no-install-recommends \
        postgresql-18-postgis-3=3.6.4+dfsg-2.pgdg13+1 \
        postgresql-18-postgis-3-scripts=3.6.4+dfsg-2.pgdg13+1

RUN set -xe ;\
    apt-get install -y --no-install-recommends postgresql-18-pgvector=0.8.7-1.pgdg13+1

RUN set -xe ;\
    wget --quiet -O /tmp/vchord.deb https://github.com/tensorchord/VectorChord/releases/download/1.1.1/postgresql-18-vchord_1.1.1-1_amd64.deb ;\
    apt-get install -y --no-install-recommends /tmp/vchord.deb ;\
    rm /tmp/vchord.deb

RUN pgxn install pgmq==1.13.1

RUN usermod -u 26 postgres

USER 26
