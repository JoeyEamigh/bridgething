import 'react-native-get-random-values';
import { AppRegistry } from 'react-native';
import App from './App';
import { name as appName } from './app.json';
import { installCrashHandlers } from './lib/crash';

installCrashHandlers();

AppRegistry.registerComponent(appName, () => App);
