#!/bin/sh
set -eu

export AWS_DEFAULT_REGION=us-east-2

if ! awslocal dynamodb describe-table --table-name moonrabbit >/dev/null 2>&1; then
  awslocal dynamodb create-table \
    --table-name moonrabbit \
    --attribute-definitions AttributeName=pk,AttributeType=S AttributeName=sk,AttributeType=S \
    --key-schema AttributeName=pk,KeyType=HASH AttributeName=sk,KeyType=RANGE \
    --billing-mode PAY_PER_REQUEST
fi

if ! awslocal s3api head-bucket --bucket moonrabbit-audio >/dev/null 2>&1; then
  awslocal s3 mb s3://moonrabbit-audio
fi
