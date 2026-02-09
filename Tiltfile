# Beacon Development Orchestration
#
# Usage:
#   tilt up    - Start all services
#   tilt down  - Stop all services

load("ext://git_resource", "git_checkout")
load("ext://dotenv", "dotenv")
load("ext://color", "color")

# Load environment
if os.path.exists(".env.local"):
    dotenv(fn=".env.local")

# Read service configuration
config_path = "services.yaml"
if not os.path.exists(config_path):
    fail("Missing services.yaml - copy from services.yaml.template")

services = read_yaml(config_path).get("services", [])

# ------------------------------------
# Platform dependencies
# ------------------------------------

# Synapse - AI router (LLM, MCP, STT, TTS)
synapse_path = "%s/projects/omni/synapse" % os.environ["HOME"]
synapse_config = os.path.abspath("config/synapse.dev.toml")

if os.path.exists(synapse_path):
    local_resource(
        "build-synapse",
        cmd="cargo build -p synapse",
        dir=synapse_path,
        deps=[
            "%s/synapse/src" % synapse_path,
            "%s/crates" % synapse_path,
            "%s/Cargo.toml" % synapse_path,
        ],
        labels=["synapse"],
    )

    local_resource(
        "dev-synapse",
        serve_cmd="cargo run -p synapse -- --config %s" % synapse_config,
        serve_dir=synapse_path,
        resource_deps=["build-synapse"],
        readiness_probe=probe(
            http_get=http_get_action(
                path="/health",
                port=6000,
            ),
            initial_delay_secs=5,
            period_secs=5,
        ),
        labels=["synapse"],
    )
    synapse_available = True
else:
    print(color.yellow("Synapse not found at %s - LLM routing will be unavailable" % synapse_path))
    synapse_available = False

# ------------------------------------
# Bootstrap services
# ------------------------------------
for service in services:
    name = service.keys()[0]
    values = service.values()[0]

    # Handle metarepos - auto-discover services in {path}/services/*/Tiltfile
    if values.get("metarepo", False):
        base_path = values.get("path", "services/%s" % name)
        repo = values.get("repo")

        # Expand ~ to home directory
        if base_path.startswith("~"):
            base_path = base_path.replace("~", os.environ["HOME"])

        # Clone if repo specified and path doesn't exist
        if repo and not os.path.exists(base_path):
            print(color.yellow("%s does not exist, cloning..." % base_path))
            git_checkout(repo, base_path)
        elif os.path.exists(base_path):
            print(color.green("%s already exists" % base_path))

        # Auto-discover services using shell
        services_dir = "%s/services" % base_path
        if os.path.exists(services_dir):
            sub_services = str(local("ls %s" % services_dir, quiet=True)).strip().split("\n")
            for sub_service in sub_services:
                if sub_service:
                    sub_path = "%s/%s" % (services_dir, sub_service)
                    tiltfile_path = "%s/Tiltfile" % sub_path
                    if os.path.exists(tiltfile_path):
                        print(color.green("     Loading Tiltfile for %s..." % sub_service))
                        include(tiltfile_path)
        continue

    repo = values.get("repo", "")
    path = values.get("path", "services/%s" % name)

    # Expand ~ to home directory
    if path.startswith("~"):
        path = path.replace("~", os.environ["HOME"])

    # Clone if repo specified and path doesn't exist
    if repo and not os.path.exists(path):
        print(color.yellow("%s does not exist, cloning..." % path))
        git_checkout(repo, path)
    elif os.path.exists(path):
        print(color.green("%s already exists" % path))

    # Include service Tiltfile if it exists
    tiltfile_path = "%s/Tiltfile" % path
    if os.path.exists(tiltfile_path):
        print(color.green("     Loading Tiltfile for %s..." % name))
        include(tiltfile_path)
    else:
        print(color.yellow("No Tiltfile found for %s at %s" % (name, tiltfile_path)))
