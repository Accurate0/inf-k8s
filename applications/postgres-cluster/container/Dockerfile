FROM ghcr.io/cloudnative-pg/postgresql:18.0-202509261031-standard-trixie

USER root
RUN set -xe ;\
    apt update && apt install build-essential pgxnclient -y;

# TimescaleDB
RUN apt-get install -y gnupg postgresql-common apt-transport-https lsb-release wget
RUN echo "deb https://packagecloud.io/timescale/timescaledb/debian/ $(lsb_release -c -s) main" \
    | tee /etc/apt/sources.list.d/timescaledb.list

RUN wget --quiet -O - https://packagecloud.io/timescale/timescaledb/gpgkey \
    | gpg --dearmor -o /etc/apt/trusted.gpg.d/timescaledb.gpg

RUN set -xe ;\
    apt update;\
    apt-get install -y timescaledb-2-postgresql-18=2.23.1~debian13;

RUN pgxn install pgmq==1.11.1

RUN usermod -u 26 postgres

USER 26
