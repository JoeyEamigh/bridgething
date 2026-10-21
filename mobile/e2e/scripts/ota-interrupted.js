var res = http.get(CONTROL + '/control/daemon/interrupted');
if (res.status !== 200) throw 'the link was not dropped under the image stream: ' + res.body;
