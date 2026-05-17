(function (root, factory) {
    if (typeof define === 'function' && define.amd) {
        // AMD
        define(['leaflet', 'maplibre-gl'], factory);
    } else if (typeof exports === 'object') {
        // Node, CommonJS-like
        module.exports = factory(require('leaflet'), require('maplibre-gl'));
    } else {
        // Browser globals (root is window)
        root.returnExports = factory(root.L, root.maplibregl);
    }
}(typeof globalThis !== 'undefined' ? globalThis : this || self, function (L, maplibregl) {
    L.MaplibreGL = L.Layer.extend({
        options: {
            updateInterval: 32,
            // How much to extend the overlay view (relative to map size)
            // e.g. 0.1 would be 10% of map view in each direction
            padding: 0.1,
            // whether or not to register the mouse and keyboard
            // events on the maplibre overlay
            interactive: false,
            // set the tilepane as the default pane to draw gl tiles
            pane: 'tilePane'
        },

        initialize: function (options) {
            L.setOptions(this, options);

            // setup throttling the update event when panning
            this._throttledUpdate = L.Util.throttle(this._update, this.options.updateInterval, this);
        },

        onAdd: function (map) {
            if (!this._container) {
                this._initContainer();
            }

            var paneName = this.getPaneName();
            map.getPane(paneName).appendChild(this._container);

            this._initGL();

            this._offset = this._map.containerPointToLayerPoint([0, 0]);

            // work around https://github.com/mapbox/mapbox-gl-leaflet/issues/47
            if (map.options.zoomAnimation) {
                L.DomEvent.on(map._proxy, L.DomUtil.TRANSITION_END, this._transitionEnd, this);
            }
        },

        onRemove: function (map) {
            if (this._map._proxy && this._map.options.zoomAnimation) {
                L.DomEvent.off(this._map._proxy, L.DomUtil.TRANSITION_END, this._transitionEnd, this);
            }
            var paneName = this.getPaneName();
            map.getPane(paneName).removeChild(this._container);

            this._glMap.remove();
            this._glMap = null;
        },

        getEvents: function () {
            return {
                move: this._throttledUpdate, // sensibly throttle updating while panning
                zoomanim: this._animateZoom, // applys the zoom animation to the <canvas>
                zoom: this._pinchZoom, // animate every zoom event for smoother pinch-zooming
                zoomstart: this._zoomStart, // flag starting a zoom to disable panning
                zoomend: this._zoomEnd,
                resize: this._resize
            };
        },

        // https://leafletjs.com/reference.html#layer-getattribution
        getAttribution: function () {
            // Return custom attribution if specified in options
            if (this.options.attributionControl) {
                return this.options.attributionControl.customAttribution;
            }

            // Gather attributions from MapLibre styles
            var map = this._glMap;
            if (map && this.options.attributionControl !== false) {
                var style = map.getStyle();
                if (style && style.sources) {
                    return Object.keys(style.sources)
                        .map(function (sourceId) {
                            var source = map.getSource(sourceId);
                            return (source && typeof source.attribution === 'string') ? source.attribution.trim() : null;
                        })
                        .filter(Boolean) // Remove null/undefined values
                        .join(', ');
                }
            }

            return '';
        },

        getMaplibreMap: function () {
            return this._glMap;
        },

        getCanvas: function () {
            return this._glMap.getCanvas();
        },

        getSize: function () {
            return this._map.getSize().multiplyBy(1 + this.options.padding * 2);
        },

        getBounds: function () {
            var halfSize = this.getSize().multiplyBy(0.5);
            var center = this._map.latLngToContainerPoint(this._map.getCenter());
            return L.latLngBounds(
                this._map.containerPointToLatLng(center.subtract(halfSize)),
                this._map.containerPointToLatLng(center.add(halfSize))
            );
        },

        getContainer: function () {
            return this._container;
        },

        // returns the pane name set in options if it is a valid pane, defaults to tilePane
        getPaneName: function () {
            return this._map.getPane(this.options.pane) ? this.options.pane : 'tilePane';
        },

        _roundPoint: function (p) {
            return { x: Math.round(p.x), y: Math.round(p.y) };
        },

        _initContainer: function () {
            var container = this._container = L.DomUtil.create('div', 'leaflet-gl-layer');
            this._resizeContainer();

            var offset = this._map.getSize().multiplyBy(this.options.padding);

            var topLeft = this._map.containerPointToLayerPoint([0, 0]).subtract(offset);

            L.DomUtil.setPosition(container, this._roundPoint(topLeft));
        },

        _resizeContainer: function () {
            var size = this.getSize();
            this._container.style.width = size.x + 'px';
            this._container.style.height = size.y + 'px';
        },

        _initGL: function () {
            var center = this._map.getCenter();

            var options = L.extend({}, this.options, {
                container: this._container,
                center: [center.lng, center.lat],
                zoom: this._map.getZoom() - 1,
                attributionControl: false
            });

            this._glMap = new maplibregl.Map(options);

            var _map = this._map;
            var _currentAttribution = this.getAttribution();
            var _getAttribution = this.getAttribution.bind(this);
            this._glMap.on('load', function () {
                // Force attribution update
                if (_map && _map.attributionControl) {
                    _map.attributionControl.removeAttribution(_currentAttribution);
                    _map.attributionControl.addAttribution(_getAttribution());
                }
            });

            // allow GL base map to pan beyond min/max latitudes
            // Defensively check if properties are writable before setting them,
            // ensuring compatibility with both old and new versions of MapLibre GL JS.
            var transformProto = Object.getPrototypeOf(this._glMap.transform);

            var latRangeDescriptor = Object.getOwnPropertyDescriptor(transformProto, 'latRange');
            if (!latRangeDescriptor || latRangeDescriptor.set || latRangeDescriptor.writable) {
                this._glMap.transform.latRange = null;
            }

            // Although this property is obsolete in modern versions, we apply the same
            // defensive check for robust backward compatibility.
            var maxValidLatitudeDescriptor = Object.getOwnPropertyDescriptor(transformProto, 'maxValidLatitude');
            if (!maxValidLatitudeDescriptor || maxValidLatitudeDescriptor.set || maxValidLatitudeDescriptor.writable) {
                this._glMap.transform.maxValidLatitude = Infinity;
            }


            // check for the existence of _helper and _latRange in MapLibre
            // this supports MapLibre v5
            if (this._glMap.transform._helper && this._glMap.transform._helper._latRange) {
                this._glMap.transform._helper._latRange = [-Infinity, Infinity];
            }

            this._transformGL(this._glMap);

            if (this._glMap._canvas.canvas) {
                // older versions of mapbox-gl surfaced the canvas differently
                this._glMap._actualCanvas = this._glMap._canvas.canvas;
            } else {
                this._glMap._actualCanvas = this._glMap._canvas;
            }


            // treat child <canvas> element like L.ImageOverlay
            var canvas = this._glMap._actualCanvas;
            L.DomUtil.addClass(canvas, 'leaflet-image-layer');
            L.DomUtil.addClass(canvas, 'leaflet-zoom-animated');

            if (this.options.interactive) {
                L.DomUtil.addClass(canvas, 'leaflet-interactive');
            }
            if (this.options.className) {
                L.DomUtil.addClass(canvas, this.options.className);
            }
        },

        _update: function (e) {
            if (!this._map) {
                return;
            }
            // update the offset so we can correct for it later when we zoom
            this._offset = this._map.containerPointToLayerPoint([0, 0]);

            if (this._zooming) {
                return;
            }

            var container = this._container,
                gl = this._glMap,
                offset = this._map.getSize().multiplyBy(this.options.padding),
                topLeft = this._map.containerPointToLayerPoint([0, 0]).subtract(offset);

            L.DomUtil.setPosition(container, this._roundPoint(topLeft));

            this._transformGL(gl);
        },

        _transformGL: function (gl) {
            var center = this._map.getCenter();

            // gl.setView([center.lat, center.lng], this._map.getZoom() - 1, 0);
            // calling setView directly causes sync issues because it uses requestAnimFrame

            var tr = gl._getTransformForUpdate(); // .clone() ?

            if (tr.setCenter) {
                // maplibre 5.0.0 and higher:
                tr.setCenter(maplibregl.LngLat.convert([center.lng, center.lat]));
                tr.setZoom(this._map.getZoom() - 1);
                gl.transform.apply(tr);
            } else {
                // maplibre < 5.0.0
                tr = gl.transform;
                tr.center = maplibregl.LngLat.convert([center.lng, center.lat]);
                tr.zoom = this._map.getZoom() - 1;
            }

            gl._fireMoveEvents();
        },

        // update the map constantly during a pinch zoom
        _pinchZoom: function (e) {
            this._glMap.jumpTo({
                zoom: this._map.getZoom() - 1,
                center: this._map.getCenter()
            });
        },

        // borrowed from L.ImageOverlay
        // https://github.com/Leaflet/Leaflet/blob/master/src/layer/ImageOverlay.js#L139-L144
        _animateZoom: function (e) {
            var scale = this._map.getZoomScale(e.zoom);
            var padding = this._map.getSize().multiplyBy(this.options.padding * scale);
            var viewHalf = this.getSize()._divideBy(2);
            // corrections for padding (scaled), adapted from
            // https://github.com/Leaflet/Leaflet/blob/master/src/map/Map.js#L1490-L1508
            var topLeft = this._map.project(e.center, e.zoom)
                ._subtract(viewHalf)
                ._add(this._map._getMapPanePos()
                    .add(padding))._round();
            var offset = this._map.project(this._map.getBounds().getNorthWest(), e.zoom)
                ._subtract(topLeft);

            L.DomUtil.setTransform(
                this._glMap._actualCanvas,
                offset.subtract(this._offset),
                scale
            );
        },

        _zoomStart: function (e) {
            this._zooming = true;
        },

        _zoomEnd: function () {
            var scale = this._map.getZoomScale(this._map.getZoom());

            L.DomUtil.setTransform(
                this._glMap._actualCanvas,
                // https://github.com/mapbox/mapbox-gl-leaflet/pull/130
                null,
                scale
            );

            this._zooming = false;

            this._update();
        },

        _transitionEnd: function (e) {
            L.Util.requestAnimFrame(function () {
                var zoom = this._map.getZoom();
                var center = this._map.getCenter();
                var offset = this._map.latLngToContainerPoint(
                    this._map.getBounds().getNorthWest()
                );
                this._resizeContainer();

                // reset the scale and offset
                L.DomUtil.setTransform(this._glMap._actualCanvas, offset, 1);

                // enable panning once the gl map is ready again
                this._glMap.once('moveend', L.Util.bind(function () {
                    this._zoomEnd();
                }, this));

                // update the map position
                this._glMap.jumpTo({
                    center: center,
                    zoom: zoom - 1
                });
            }, this);
        },

        _resize: function (e) {
            this._transitionEnd(e);
        }
    });

    L.maplibreGL = function (options) {
        return new L.MaplibreGL(options);
    };

}));
