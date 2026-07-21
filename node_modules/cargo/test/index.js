!function(root) {
  var common = typeof module != 'undefined' && !!module.exports
  var aok = common ? require('../node_modules/aok') : root.aok
  var cargo = common ? require('../src') : root.cargo
  if (![].some) aok.prototype.express = aok.info

  aok.pass(['local', 'session', 'temp'], function(type) {
    var api = cargo[type], id = '.' + type
    aok.info(id + '.stores: ' + api.stores)
    aok(id + '.stores is boolean', typeof api.stores == 'boolean')
    aok('.encode', false === api.encode || typeof api.encode({}) == 'string')
    aok('.decode', false === api.decode || typeof api.decode('{}') == 'object')

    aok.fail(['get', 'set', 'remove'], function(method) {
      var sub = api[method], exists = typeof sub == 'function' || sub === false
      aok([id, method].join('.'), exists)
      return exists
    }) || aok(id, function() {
      var k = id
      api.set(k, k)
      if (k !== api.get(k)) return false
      api.remove(k)
      if (null != api.get(k)) return false
      if (k !== api(k, k) || k !== api(k)) return false
      api(k, void 0) // should delegate to .remove
      if (null != api(k)) return false
      return '[object Object]' === aok.explain(api())
    })
  })
}(this);