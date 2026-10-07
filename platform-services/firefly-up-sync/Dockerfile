FROM node:26.10.0-alpine3.24

WORKDIR /opt/firefly-up-sync

COPY package.json ./
COPY src ./src

USER 10001

ENTRYPOINT ["node", "src/main.ts"]
