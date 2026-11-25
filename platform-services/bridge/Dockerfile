FROM node:26.12.1-alpine3.24 AS deps

WORKDIR /opt/bridge
RUN npm install -g pnpm
COPY package.json pnpm-lock.yaml ./
RUN pnpm install --prod --frozen-lockfile

FROM node:26.12.1-alpine3.24

WORKDIR /opt/bridge

COPY --from=deps /opt/bridge/node_modules ./node_modules
COPY package.json subgraph.graphql ./
COPY src ./src

USER 10001

ENTRYPOINT ["node", "src/main.ts"]
