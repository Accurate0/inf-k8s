FROM node:24-slim AS builder

ENV PNPM_HOME=/pnpm
ENV PATH=$PNPM_HOME:$PATH
RUN corepack enable

WORKDIR /app
COPY . .

RUN pnpm install --frozen-lockfile
RUN pnpm --filter @accurate0/feature-flag-client build
RUN pnpm --filter feature-flags-frontend build
RUN pnpm --filter feature-flags-frontend deploy --prod /app/deploy

FROM node:24-slim AS final

RUN groupadd --gid 10001 appuser && useradd --uid 10001 --gid 10001 --no-create-home appuser

WORKDIR /app
COPY --from=builder /app/frontend/build ./build
COPY --from=builder /app/deploy/node_modules ./node_modules

ENV NODE_ENV=production
ENV PORT=3000
ENV HOST=0.0.0.0
EXPOSE 3000
USER appuser

ENTRYPOINT ["node", "build"]
