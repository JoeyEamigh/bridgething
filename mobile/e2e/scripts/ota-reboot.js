var onto = [];
if (typeof IMAGE !== 'undefined') onto.push('image=' + IMAGE);
if (typeof DAEMON !== 'undefined') onto.push('daemon=' + DAEMON);
var res = http.get(CONTROL + '/control/daemon/reboot?' + onto.join('&'));
if (res.status !== 200) throw 'could not reboot the device onto ' + onto.join(' ') + ': ' + res.status + ' ' + res.body;
