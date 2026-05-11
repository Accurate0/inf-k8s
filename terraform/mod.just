[private]
default:
    @just --list tf

terraform_dir := justfile_directory() / "terraform"

# Run terraform plan
plan:
    cd {{terraform_dir}} && terraform plan

# Run terraform apply
apply:
    cd {{terraform_dir}} && terraform apply

# Run terraform init
init:
    cd {{terraform_dir}} && terraform init
