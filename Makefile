SHELL := /bin/bash
C := docker compose
T := docker compose -p famweather-test -f docker-compose.yml -f docker-compose.test.yml
ADMIN := $(C) --profile tools run --rm -T admin admin
# run a test-project command, always tear the project down, keep the command's exit code
TESTRUN = $(T) build api && { $(1); rc=$$?; ./ops/local/check-secret-logs.sh "$(T)" || rc=1; $(T) down -v --remove-orphans >/dev/null 2>&1; exit $$rc; }

JOB := $(C) run --rm --no-deps poller poller run-job

envval = $(shell grep -E '^$(1)=' .env 2>/dev/null | head -1 | cut -d= -f2-)
.PHONY: init build web-build up down logs logs-pretty ps migrate seed invite reset-link keygen add-station e2e-data psql web-dev \
        poller-replay poller-idle live check-local-only check-api-features test test-backend test-web test-db test-e2e smoke reset

# `up` does not use `up -d --wait`: Compose 5.x counts an exited one-shot (auth-access, migrate) as a failed wait even
# when it exited 0 (05 section 4.8 fallback), so ops/local/wait-up.sh checks health and one-shot exit codes itself.
init:            ; ./ops/local/init-env.sh
build:           ; $(C) build api
web-build:       ; $(C) --profile tools run --rm web
up: build
	$(C) up -d
	./ops/local/wait-up.sh
down:            ; $(C) down
logs:            ; $(C) logs -f --tail=100
# One readable line per event; SVC=poller (default) or api, auth, ...
logs-pretty:     ; $(C) logs -f --tail=100 --no-log-prefix $(or $(SVC),poller) | jq --unbuffered -Rr -f ops/local/pretty-logs.jq
ps:              ; $(C) ps -a
migrate:         ; $(C) run --rm migrate
seed:            ; $(ADMIN) seed-dev
invite:          ; @test -n "$(EMAIL)" || (echo "usage: make invite EMAIL=you@example.com NAME='You' [FOLLOW_ALL=1]"; exit 1)
	$(ADMIN) invite --email "$(EMAIL)" --display-name "$(NAME)" $(if $(FOLLOW_ALL),--follow-all)
reset-link:      ; @test -n "$(EMAIL)" || (echo "usage: make reset-link EMAIL=you@example.com"; exit 1)
	$(ADMIN) reset-link --email "$(EMAIL)"
keygen:          ; $(C) --profile tools run --rm --no-deps -T admin admin keygen
add-station:     ; $(C) --profile tools run --rm admin admin add-station $(ARGS)
e2e-data:        ; ./ops/local/e2e-data.sh
psql:            ; $(C) exec db psql -U postgres -d postgres
# Host steps need the Node major pinned in web/.nvmrc (engine-strict); ops/local/web-node.sh finds it and
# prints a directory for PATH (empty when the current node already matches).
web-dev:
	@d="$$(./ops/local/web-node.sh)" && cd web && PATH="$${d:+$$d:}$$PATH" npm ci && PATH="$${d:+$$d:}$$PATH" PUBLIC_SUPABASE_URL=http://localhost:5173 PUBLIC_SUPABASE_ANON_KEY='$(call envval,PUBLIC_SUPABASE_ANON_KEY)' npm run dev
# D6: the poller is off by default (it logs one line and touches its heartbeat). These switch the scheduler on in replay
# mode (recorded fixtures, shifted to now) and back off. `poller run-job` (e2e-data) works in either state.
poller-replay:   ; POLLER_ENABLED=true AWN_MODE=replay $(C) up -d --no-deps --force-recreate poller
poller-idle:     ; POLLER_ENABLED=false $(C) up -d --no-deps --force-recreate poller
live:           ; @test "$(call envval,AWN_APPLICATION_KEY)" != "REPLACE_WITH_DEV_APPLICATION_KEY" || (echo "set your DEV AWN application key in .env"; exit 1)
	@echo "LIVE MODE: the AWN API (use only your own station key), ACIS (baseline lookups) and Open-Meteo (forecasts) all go to the real internet services. Ctrl-C now to abort." && sleep 5
	AWN_MODE=live POLLER_ENABLED=true $(C) up -d --no-deps --force-recreate api poller
# Feature isolation (00 Testing harness): the api binary must not see decrypt, test-support or testdb.
check-api-features:
	@cd backend && if cargo tree -p api -e normal,build,features -f '{p} {f}' | grep -E 'decrypt|test-support|testdb'; then echo "api enables a forbidden core feature"; exit 1; else echo "api feature isolation ok"; fi
check-local-only: ; ! grep -rEi 'supabase\.co|fly\.dev|netlify\.app|amazonaws\.com|healthchecks\.io|hc-ping' .env.example docker-compose.yml docker-compose.test.yml $(wildcard ops/caddy) ops/local ops/db/init
test-db: init
	$(call TESTRUN,$(T) up -d --wait db && $(T) exec -T -u postgres -e MIGRATIONS_DIR=/work/backend/migrations -e OPS_DIR=/work/ops/db db sh /work/ops/db/tests/run.sh)
test-backend: init
	$(call TESTRUN,$(T) run --rm test-backend)
test-web: init
	@if [ -f web/package.json ]; then \
	{ $(T) run --rm web sh -c "mkdir -p /work/web && cd /src/web && tar cf - --exclude=./node_modules --exclude=./build --exclude=./.svelte-kit --exclude=./test-results --exclude=./playwright-report --exclude=./tests/.auth . | tar xf - -C /work/web && cd /work/web && npm ci && npm run lint && npm run check && npm run test && npm run test:tz && npm run build && node scripts/check-build.mjs && npm run test:bundle && npm run headers:check"; rc=$$?; ./ops/local/check-secret-logs.sh "$(T)" || rc=1; $(T) down -v --remove-orphans >/dev/null 2>&1; exit $$rc; }; \
	else echo "test-web: web/package.json absent, skipping"; fi
test:            ; $(MAKE) check-local-only test-db test-backend test-web
test-e2e:        ; @test "$(CONFIRM)" = 1 || (echo "test-e2e RESETS the working stack (make reset). Re-run: make test-e2e CONFIRM=1"; exit 1)
	$(MAKE) reset up web-build seed e2e-data
	@d="$$(./ops/local/web-node.sh)" && cd web && export PATH="$${d:+$$d:}$$PATH" && echo "e2e on node $$(node -v)" && npm ci && npx playwright install chromium && E2E_BASE_URL=http://localhost:8000 npm run test:e2e
smoke:           ; ./ops/local/smoke.sh $(SMOKE_ARGS)
reset:           ; $(C) --profile tools down -v --remove-orphans
