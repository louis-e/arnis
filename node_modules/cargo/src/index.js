!function(root, name, make) {
  if (typeof module != 'undefined' && module.exports) module.exports = make()
  else root[name] = make()
}(this, 'cargo', function() {

  var cargo = {}
    , win = typeof window != 'undefined' && window
    , son = typeof JSON != 'undefined' && JSON || false
    , has = {}.hasOwnProperty
    
  function clone(o) {
    var k, r = {}
    for (k in o) has.call(o, k) && (r[k] = o[k])
    return r
  }
  
  function test(api, key) {
    if (api) try {
      key = key || 'cargo'+-new Date
      api.setItem(key, key)
      api.removeItem(key)
      return true
    } catch (e) {}
    return false
  }
  
  /**
   * @param {Storage=} api
   * @return {Function} abstraction
   */
  function abstracts(api) {
    var und, stores = test(api), cache = {}, all = stores ? api : cache
    function f(k, v) {
      var n = arguments.length
      if (1 < n) return und === v ? f['remove'](k) : f['set'](k, v), v
      return n ? f['get'](k) : clone(all)
    }
    f['stores'] = stores
    f['decode'] = son.parse
    f['encode'] = son.stringify
    f['get'] = stores ? function(k) {
      return und == (k = api.getItem(k)) ? und : k
    } : function(k) {
      return !has.call(cache, k) ? und : cache[k]
    }
    f['set'] = stores ? function(k, v) {
      api.setItem(k, v)
    } : function(k, v) {
      cache[k] = v
    }
    f['remove'] = stores ? function(k) {
      api.removeItem(k)
    } : function(k) {
      delete cache[k]
    }
    return f
  }

  cargo['session'] = abstracts(win.sessionStorage)
  cargo['local'] = abstracts(win.localStorage)
  cargo['temp'] = abstracts()
  return cargo
});