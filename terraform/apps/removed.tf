removed {
  from = cloudflare_r2_bucket.bom-images

  lifecycle {
    destroy = false
  }
}
