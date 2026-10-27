const path = require('path');
const { getDefaultConfig } = require('@react-native/metro-config');
const { withNativeWind } = require('nativewind/metro');
const {
  wrapWithReanimatedMetroConfig,
} = require('react-native-reanimated/metro-config');

const projectRoot = __dirname;
const workspaceRoot = path.resolve(projectRoot, '..');

const config = getDefaultConfig(projectRoot);

config.watchFolders = [workspaceRoot];
config.resolver.nodeModulesPaths = [
  path.resolve(projectRoot, 'node_modules'),
  path.resolve(workspaceRoot, 'node_modules'),
];

const portalEntry = require.resolve('@rn-primitives/portal');
const reactNativeDir = path.dirname(
  require.resolve('react-native/package.json'),
);
const assetRegistry = require.resolve(
  '@react-native/assets-registry/registry',
  {
    paths: [reactNativeDir],
  },
);
const baseResolveRequest = config.resolver.resolveRequest;
config.resolver.resolveRequest = (context, moduleName, platform) => {
  if (moduleName === '@rn-primitives/portal') {
    return { type: 'sourceFile', filePath: portalEntry };
  }
  if (moduleName === '@react-native/assets-registry/registry') {
    return { type: 'sourceFile', filePath: assetRegistry };
  }
  return baseResolveRequest
    ? baseResolveRequest(context, moduleName, platform)
    : context.resolveRequest(context, moduleName, platform);
};

module.exports = wrapWithReanimatedMetroConfig(
  withNativeWind(config, { input: './global.css', inlineRem: 16 }),
);
