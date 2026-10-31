import {
  to = module.apps.cloudflare_r2_bucket.registry-storage
  id = "${local.cloudflare_account_id}/registry/default"
}

import {
  to = module.apps.cloudflare_r2_bucket.k8s-schemas-schemas
  id = "${local.cloudflare_account_id}/k8s-schemas/default"
}

import {
  to = module.apps.cloudflare_r2_bucket.maccas-api-images
  id = "${local.cloudflare_account_id}/maccas-images/default"
}

moved {
  from = aws_s3_bucket.home-gateway-bucket
  to   = module.apps.aws_s3_bucket.home-gateway-storage
}

moved {
  from = aws_s3_bucket_public_access_block.home-gateway-bucket-public-block
  to   = module.apps.aws_s3_bucket_public_access_block.home-gateway-storage
}

moved {
  from = aws_iam_user.home-gateway-user
  to   = module.apps.aws_iam_user.home-gateway-storage
}

moved {
  from = aws_iam_policy.home-gateway-policy
  to   = module.apps.aws_iam_policy.home-gateway-storage
}

moved {
  from = aws_iam_user_policy_attachment.policy-attachment
  to   = module.apps.aws_iam_user_policy_attachment.home-gateway-storage
}

moved {
  from = aws_iam_access_key.home-gateway-access-key
  to   = module.apps.aws_iam_access_key.home-gateway-storage
}
