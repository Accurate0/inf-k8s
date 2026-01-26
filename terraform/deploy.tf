module "aws-oidc-deploy" {
  source = "./modules/aws-oidc-deploy"
  name   = "inf-k8s"
  resource_access_policy = {
    "Version" = "2012-10-17"

    "Statement" = [
      {
        "Effect" = "Allow",
        "Action" = [
          "lambda:UpdateFunctionCode",
        ],
        "Resource" = module.object-registry.aws_lambda_arns
      },
    ]
  }
  allowed_repos = ["inf-k8s"]
}
