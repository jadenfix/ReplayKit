FROM node:22-bookworm-slim

WORKDIR /app

COPY apps/web/package.json apps/web/package-lock.json ./
RUN npm ci

COPY apps/web/ ./

ARG VITE_REPLAYKIT_API_BASE_URL=http://localhost:3210
ENV VITE_REPLAYKIT_API_BASE_URL=$VITE_REPLAYKIT_API_BASE_URL

RUN npm run build

EXPOSE 5173

CMD ["npx", "vite", "preview", "--host", "0.0.0.0", "--port", "5173"]
