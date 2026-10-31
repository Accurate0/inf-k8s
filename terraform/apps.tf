locals {
  app_files = fileset("${path.module}/..", "{projects,platform-services}/*/app.yaml")
  apps      = [for file in local.app_files : yamldecode(file("${path.module}/../${file}"))]

  app_databases = merge([
    for app in local.apps : {
      for database, spec in try(app.databases, {}) : database => {
        namespace = app.name
        role      = try(spec.databaseName, replace(database, "-", "_"))
      }
    }
  ]...)
}
