import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { App, CfnOutput, Duration, RemovalPolicy, Stack, type StackProps } from 'aws-cdk-lib';
import { HttpApi, HttpMethod } from 'aws-cdk-lib/aws-apigatewayv2';
import { HttpJwtAuthorizer } from 'aws-cdk-lib/aws-apigatewayv2-authorizers';
import { HttpLambdaIntegration } from 'aws-cdk-lib/aws-apigatewayv2-integrations';
import {
  AllowedMethods,
  CachePolicy,
  Function as CloudFrontFunction,
  Distribution,
  FunctionCode,
  FunctionEventType,
  FunctionRuntime,
  HeadersFrameOption,
  HeadersReferrerPolicy,
  OriginProtocolPolicy,
  OriginRequestPolicy,
  ResponseHeadersPolicy,
  ViewerProtocolPolicy,
} from 'aws-cdk-lib/aws-cloudfront';
import { HttpOrigin, S3BucketOrigin } from 'aws-cdk-lib/aws-cloudfront-origins';
import {
  AccountRecovery,
  CfnManagedLoginBranding,
  FeaturePlan,
  ManagedLoginVersion,
  Mfa,
  OAuthScope,
  UserPool,
  type UserPoolClient,
} from 'aws-cdk-lib/aws-cognito';
import { AttributeType, BillingMode, Table } from 'aws-cdk-lib/aws-dynamodb';
import { Architecture, Code, Function as LambdaFunction, Runtime } from 'aws-cdk-lib/aws-lambda';
import { LogGroup, RetentionDays } from 'aws-cdk-lib/aws-logs';
import { BlockPublicAccess, Bucket, BucketEncryption } from 'aws-cdk-lib/aws-s3';
import { BucketDeployment, CacheControl, Source } from 'aws-cdk-lib/aws-s3-deployment';
import {
  AwsCustomResource,
  AwsCustomResourcePolicy,
  PhysicalResourceId,
} from 'aws-cdk-lib/custom-resources';
import { Trigger } from 'aws-cdk-lib/triggers';

class MoonRabbitStack extends Stack {
  constructor(scope: App, id: string, props: StackProps & { readonly stage: string }) {
    super(scope, id, props);
    const stage = props.stage;

    const openRouterKey = process.env['OPENROUTER_API_KEY'];
    if (!openRouterKey) {
      throw new Error('OPENROUTER_API_KEY is not set.');
    }

    const userPool = new UserPool(this, 'Users', {
      userPoolName: `MoonRabbit-${stage}`,
      selfSignUpEnabled: false,
      signInAliases: { email: true },
      autoVerify: { email: true },
      standardAttributes: { email: { required: true, mutable: true } },
      passwordPolicy: {
        minLength: 12,
        requireLowercase: true,
        requireUppercase: true,
        requireDigits: true,
        requireSymbols: true,
        tempPasswordValidity: Duration.days(7),
      },
      accountRecovery: AccountRecovery.EMAIL_ONLY,
      mfa: Mfa.OPTIONAL,
      mfaSecondFactor: { otp: true, sms: false },
      featurePlan: FeaturePlan.ESSENTIALS,
      removalPolicy: RemovalPolicy.RETAIN,
    });

    const domainPrefix = `moonrabbit-${stage}-${this.account}`;
    const domain = userPool.addDomain('HostedUi', {
      cognitoDomain: { domainPrefix },
      managedLoginVersion: ManagedLoginVersion.NEWER_MANAGED_LOGIN,
    });

    const client = userPool.addClient('Web', {
      generateSecret: false,
      authFlows: { userSrp: true },
      oAuth: {
        flows: { authorizationCodeGrant: true },
        scopes: [OAuthScope.OPENID, OAuthScope.EMAIL, OAuthScope.PROFILE],
        callbackUrls: ['https://placeholder.invalid/auth/callback'],
        logoutUrls: ['https://placeholder.invalid/'],
      },
      preventUserExistenceErrors: true,
      enableTokenRevocation: true,
      accessTokenValidity: Duration.minutes(60),
      idTokenValidity: Duration.minutes(60),
      refreshTokenValidity: Duration.days(30),
      refreshTokenRotationGracePeriod: Duration.seconds(0),
    });

    const branding = new CfnManagedLoginBranding(this, 'ManagedLogin', {
      userPoolId: userPool.userPoolId,
      clientId: client.userPoolClientId,
      useCognitoProvidedValues: true,
    });
    branding.node.addDependency(domain);

    const table = new Table(this, 'Table', {
      partitionKey: { name: 'pk', type: AttributeType.STRING },
      sortKey: { name: 'sk', type: AttributeType.STRING },
      billingMode: BillingMode.PAY_PER_REQUEST,
      pointInTimeRecoverySpecification: { pointInTimeRecoveryEnabled: true },
      removalPolicy: RemovalPolicy.RETAIN,
    });

    const audio = new Bucket(this, 'Audio', {
      blockPublicAccess: BlockPublicAccess.BLOCK_ALL,
      encryption: BucketEncryption.S3_MANAGED,
      enforceSSL: true,
      removalPolicy: RemovalPolicy.RETAIN,
    });

    const logs = new LogGroup(this, 'ApiLogs', {
      retention: RetentionDays.ONE_MONTH,
      removalPolicy: RemovalPolicy.DESTROY,
    });

    const code = Code.fromAsset(join(__dirname, '../../api/target/lambda/api'));
    const environment = {
      TABLE_NAME: table.tableName,
      BUCKET_NAME: audio.bucketName,
      OPENROUTER_API_KEY: openRouterKey,
      RUST_LOG: 'info',
    };

    const api = new LambdaFunction(this, 'Api', {
      runtime: Runtime.PROVIDED_AL2023,
      architecture: Architecture.ARM_64,
      handler: 'bootstrap',
      code,
      memorySize: 1769,
      timeout: Duration.seconds(29),
      logGroup: logs,
      environment,
    });
    table.grantReadWriteData(api);
    audio.grantReadWrite(api);

    const seeder = new LambdaFunction(this, 'Seeder', {
      runtime: Runtime.PROVIDED_AL2023,
      architecture: Architecture.ARM_64,
      handler: 'bootstrap',
      code,
      memorySize: 1769,
      timeout: Duration.minutes(5),
      environment: { ...environment, SEED_ONLY: '1' },
    });
    table.grantReadWriteData(seeder);
    new Trigger(this, 'SeedCatalog', {
      handler: seeder,
      timeout: Duration.minutes(5),
    });

    const httpApi = new HttpApi(this, 'HttpApi');
    httpApi.addRoutes({
      path: '/api/{proxy+}',
      methods: [HttpMethod.ANY],
      integration: new HttpLambdaIntegration('Api', api),
      authorizer: new HttpJwtAuthorizer(
        'Users',
        `https://cognito-idp.${this.region}.amazonaws.com/${userPool.userPoolId}`,
        { jwtAudience: [client.userPoolClientId] },
      ),
    });

    const site = new Bucket(this, 'Site', {
      blockPublicAccess: BlockPublicAccess.BLOCK_ALL,
      encryption: BucketEncryption.S3_MANAGED,
      enforceSSL: true,
      removalPolicy: RemovalPolicy.DESTROY,
      autoDeleteObjects: true,
    });

    const hostedUi = `https://${domainPrefix}.auth.${this.region}.amazoncognito.com`;
    const headers = new ResponseHeadersPolicy(this, 'SecurityHeaders', {
      securityHeadersBehavior: {
        strictTransportSecurity: {
          accessControlMaxAge: Duration.days(365),
          includeSubdomains: true,
          override: true,
        },
        contentTypeOptions: { override: true },
        frameOptions: { frameOption: HeadersFrameOption.DENY, override: true },
        referrerPolicy: {
          referrerPolicy: HeadersReferrerPolicy.STRICT_ORIGIN_WHEN_CROSS_ORIGIN,
          override: true,
        },
        contentSecurityPolicy: {
          contentSecurityPolicy: [
            "default-src 'self'",
            "img-src 'self' data: blob:",
            "media-src 'self' blob:",
            `connect-src 'self' https://cognito-idp.${this.region}.amazonaws.com ${hostedUi}`,
            "style-src 'self'",
            "style-src-attr 'unsafe-inline'",
            "font-src 'self'",
            "script-src 'self'",
            "frame-ancestors 'none'",
            "base-uri 'self'",
            `form-action 'self' ${hostedUi}`,
          ].join('; '),
          override: true,
        },
      },
    });

    const rewrite = new CloudFrontFunction(this, 'SpaRewrite', {
      runtime: FunctionRuntime.JS_2_0,
      code: FunctionCode.fromInline(`function handler(event) {
  var request = event.request;
  if (request.uri.indexOf('.') === -1) {
    request.uri = '/index.html';
  }
  return request;
}`),
    });

    const distribution = new Distribution(this, 'Cdn', {
      defaultRootObject: 'index.html',
      defaultBehavior: {
        origin: S3BucketOrigin.withOriginAccessControl(site),
        viewerProtocolPolicy: ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
        responseHeadersPolicy: headers,
        functionAssociations: [{ function: rewrite, eventType: FunctionEventType.VIEWER_REQUEST }],
      },
      additionalBehaviors: {
        '/api/*': {
          origin: new HttpOrigin(`${httpApi.apiId}.execute-api.${this.region}.amazonaws.com`, {
            protocolPolicy: OriginProtocolPolicy.HTTPS_ONLY,
          }),
          viewerProtocolPolicy: ViewerProtocolPolicy.REDIRECT_TO_HTTPS,
          allowedMethods: AllowedMethods.ALLOW_ALL,
          cachePolicy: CachePolicy.CACHING_DISABLED,
          originRequestPolicy: OriginRequestPolicy.ALL_VIEWER_EXCEPT_HOST_HEADER,
        },
      },
    });

    const siteUrl = `https://${distribution.distributionDomainName}`;
    this.clientUrls(client, userPool.userPoolId, siteUrl);

    const webDist = join(__dirname, '../../../dist/apps/web/browser');
    const siteStamp = createHash('sha256')
      .update(readFileSync(join(webDist, 'index.html')))
      .digest('hex')
      .slice(0, 16);
    const siteFiles = new BucketDeployment(this, 'SiteFiles', {
      sources: [Source.asset(webDist, { exclude: ['config.json'] })],
      destinationBucket: site,
      distribution,
      distributionPaths: ['/*'],
    });
    const config = new BucketDeployment(this, 'RuntimeConfig', {
      sources: [
        Source.jsonData('config.json', {
          auth: {
            authority: `https://cognito-idp.${this.region}.amazonaws.com/${userPool.userPoolId}`,
            clientId: client.userPoolClientId,
            domain: hostedUi,
          },
        }),
      ],
      destinationBucket: site,
      distribution,
      distributionPaths: ['/config.json'],
      prune: false,
      cacheControl: [CacheControl.noCache()],
      metadata: { site: siteStamp },
    });
    config.node.addDependency(siteFiles);

    new CfnOutput(this, 'SiteUrl', { value: siteUrl });
    new CfnOutput(this, 'UserPoolId', { value: userPool.userPoolId });
  }

  /** Cognito's callback URL and the distribution domain would otherwise cycle. */
  private clientUrls(client: UserPoolClient, userPoolId: string, siteUrl: string): void {
    const parameters = {
      UserPoolId: userPoolId,
      ClientId: client.userPoolClientId,
      CallbackURLs: [`${siteUrl}/auth/callback`],
      LogoutURLs: [`${siteUrl}/`],
      AllowedOAuthFlows: ['code'],
      AllowedOAuthScopes: ['openid', 'email', 'profile'],
      AllowedOAuthFlowsUserPoolClient: true,
      SupportedIdentityProviders: ['COGNITO'],
      ExplicitAuthFlows: ['ALLOW_USER_SRP_AUTH'],
      PreventUserExistenceErrors: 'ENABLED',
      EnableTokenRevocation: true,
      AccessTokenValidity: 60,
      IdTokenValidity: 60,
      RefreshTokenValidity: 30,
      TokenValidityUnits: {
        AccessToken: 'minutes',
        IdToken: 'minutes',
        RefreshToken: 'days',
      },
      RefreshTokenRotation: { Feature: 'ENABLED', RetryGracePeriodSeconds: 0 },
    };
    const call = {
      service: 'CognitoIdentityServiceProvider',
      action: 'updateUserPoolClient',
      parameters,
      physicalResourceId: PhysicalResourceId.of(client.userPoolClientId),
    };
    new AwsCustomResource(this, 'ClientUrls', {
      installLatestAwsSdk: true,
      onCreate: call,
      onUpdate: call,
      policy: AwsCustomResourcePolicy.fromSdkCalls({
        resources: AwsCustomResourcePolicy.ANY_RESOURCE,
      }),
    });
  }
}

const account = process.env['CDK_DEFAULT_ACCOUNT'];
if (!account) {
  throw new Error('CDK_DEFAULT_ACCOUNT is not set.');
}

const stage = process.env['STAGE'] ?? 'prod';
if (!/^[a-z][a-z0-9-]{0,20}$/.test(stage)) {
  throw new Error('STAGE must be a short lowercase name, such as prod.');
}

const app = new App();
new MoonRabbitStack(app, `MoonRabbit-${stage}`, {
  stage,
  env: { account, region: 'us-east-2' },
});
