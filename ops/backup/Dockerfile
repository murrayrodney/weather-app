# Build context: the repo root (06 section 9.2). The root .dockerignore lets through only the files copied below.
FROM postgres:18-alpine
# pg_dump major must be >= server major; bump with Supabase's Postgres version (dependabot docker ecosystem)
RUN apk add --no-cache bash age aws-cli curl ca-certificates \
 && adduser -D -u 10001 backup
COPY ops/backup/backup.sh /usr/local/bin/backup.sh
COPY ops/db/checks/size.sql /opt/checks/size.sql
# same guard as the backend image: `.keep` keeps the build working before the CA is committed
COPY backend/certs/ /etc/ssl/app-certs/
RUN chmod 0555 /usr/local/bin/backup.sh
USER backup
ENTRYPOINT ["/usr/local/bin/backup.sh"]
