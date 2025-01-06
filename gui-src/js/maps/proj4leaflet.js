(function (factory) {
	var L, proj4;
	if (typeof define === 'function' && define.amd) {
		// AMD
		define(['leaflet', 'proj4'], factory);
	} else if (typeof module !== 'undefined') {
		// Node/CommonJS
		L = require('leaflet');
		proj4 = require('proj4');
		module.exports = factory(L, proj4);
	} else {
		// Browser globals
		if (typeof window.L === 'undefined' || typeof window.proj4 === 'undefined')
			throw "Leaflet and proj4 must be loaded first";
		factory(window.L, window.proj4);
	}
}(function (L, proj4) {

L.Proj = {};

L.Proj._isProj4Obj = function(a) {
	return (typeof a['inverse'] !== 'undefined' 
		&& typeof a['forward'] !== 'undefined');
};

L.Proj.ScaleDependantTransformation = function(scaleTransforms) {
		this.scaleTransforms = scaleTransforms;
	}

L.Proj.ScaleDependantTransformation.prototype.transform = function(point, scale) {
		return this.scaleTransforms[scale].transform(point, scale);
};

L.Proj.ScaleDependantTransformation.prototype.untransform = function(point, scale) {
		return this.scaleTransforms[scale].untransform(point, scale);
};

L.Proj.Projection = L.Class.extend({
	initialize: function(a, def) {
		if (L.Proj._isProj4Obj(a)) {
			this._proj = a;
		} else {
			var code = a;
			if (def) {
				proj4.defs(code, def);
			} else if (proj4.defs[code] === undefined) {
				var urn = code.split(':');
				if (urn.length > 3)
					code = urn[urn.length - 3] + ':' + urn[urn.length - 1];
				if (proj4.defs[code] === undefined)
					throw "No projection definition for code " + code;
			}
			this._proj = proj4(code);
		}
	},

	project: function (latlng) {
		var point = this._proj.forward([latlng.lng, latlng.lat]);
		return new L.Point(point[0], point[1]);
	},

	unproject: function (point, unbounded) {
		var point2 = this._proj.inverse([point.x, point.y]);
		return new L.LatLng(point2[1], point2[0], unbounded);
	}
});

L.Proj.CRS = L.Class.extend({
	includes: L.CRS,

	options: {
		transformation: new L.Transformation(1, 0, -1, 0)
	},

	initialize: function(a, b, c) {
		var code, proj, def, options;

		if (L.Proj._isProj4Obj(a)) {
			proj = a;
			code = proj.srsCode;
			options = b || {};

			this.projection = new L.Proj.Projection(proj);
		} else {
			code = a;
			def = b;
			options = c || {};
			this.projection = new L.Proj.Projection(code, def);
		}

		L.Util.setOptions(this, options);
		this.code = code;
		this.transformation = this.options.transformation;

		if (this.options.origin) {
			this.transformation =
				new L.Transformation(1, -this.options.origin[0],
					-1, this.options.origin[1]);
		}

		if (this.options.scales) {
			this.scale = function(zoom) {
				return this.options.scales[zoom];
			}
		} else if (this.options.resolutions) {
			this.scale = function(zoom) {
				return 1 / this.options.resolutions[zoom];
			}
		}
	}
});

L.Proj.CRS.TMS = L.Proj.CRS.extend({
	initialize: function(a, b, c, d) {
		if (L.Proj._isProj4Obj(a)) {
			var proj = a,
				projectedBounds = b,
				options = c || {};
			options.origin = [projectedBounds[0], projectedBounds[3]];
			L.Proj.CRS.prototype.initialize.call(this, proj, options);
		} else {
			var code = a,
				def = b,
				projectedBounds = c,
				options = d || {};
			options.origin = [projectedBounds[0], projectedBounds[3]];
			L.Proj.CRS.prototype.initialize.call(this, code, def, options);
		}

		this.projectedBounds = projectedBounds;
	}
});

L.Proj.TileLayer = {};

L.Proj.TileLayer.TMS = L.TileLayer.extend({
	options: {
		tms: true,
		continuousWorld: true
	},

	initialize: function(urlTemplate, crs, options) {
		var boundsMatchesGrid = true,
			scaleTransforms,
			upperY,
			crsBounds;

		if (!(crs instanceof L.Proj.CRS.TMS)) {
			throw new Error("CRS is not L.Proj.CRS.TMS.");
		}

		L.TileLayer.prototype.initialize.call(this, urlTemplate, options);
		this.crs = crs;
		crsBounds = this.crs.projectedBounds;

		// Verify grid alignment
		for (var i = this.options.minZoom; i < this.options.maxZoom && boundsMatchesGrid; i++) {
			var gridHeight = (crsBounds[3] - crsBounds[1]) /
				this._projectedTileSize(i);
			boundsMatchesGrid = Math.abs(gridHeight - Math.round(gridHeight)) > 1e-3;
		}

		if (!boundsMatchesGrid) {
			scaleTransforms = {};
			for (var i = this.options.minZoom; i < this.options.maxZoom; i++) {
				upperY = crsBounds[1] + Math.ceil((crsBounds[3] - crsBounds[1]) /
					this._projectedTileSize(i)) * this._projectedTileSize(i);
				scaleTransforms[this.crs.scale(i)] = new L.Transformation(1, -crsBounds[0], -1, upperY);
			}

			this.crs = new L.Proj.CRS.TMS(this.crs.projection._proj, crsBounds, this.crs.options);
			this.crs.transformation = new L.Proj.ScaleDependantTransformation(scaleTransforms);
		}
	},

	getTileUrl: function(tilePoint, zoom) {
		var gridHeight = Math.ceil(
      (this.crs.projectedBounds[3] - this.crs.projectedBounds[1]) /
      this._projectedTileSize(zoom)
    );

		return L.Util.template(this._url, L.Util.extend({
			s: this._getSubdomain(tilePoint),
			z: zoom,
			x: tilePoint.x,
			y: gridHeight - tilePoint.y - 1
		}, this.options));
	},

	_projectedTileSize: function(zoom) {
		return (this.options.tileSize / this.crs.scale(zoom));
	}
});

L.Proj.GeoJSON = L.GeoJSON.extend({
	initialize: function(geojson, options) {
		if (geojson.crs && geojson.crs.type == 'name') {
			var crs = new L.Proj.CRS(geojson.crs.properties.name)
			options = options || {};
			options.coordsToLatLng = function(coords) {
				var point = L.point(coords[0], coords[1]);
				return crs.projection.unproject(point);
			};
		}
		L.GeoJSON.prototype.initialize.call(this, geojson, options);
	}
});

L.Proj.geoJson = function(geojson, options) {
	return new L.Proj.GeoJSON(geojson, options);
};

if (typeof L.CRS !== 'undefined') {
	// This is left here for backwards compatibility
	L.CRS.proj4js = (function () {
		return function (code, def, transformation, options) {
			options = options || {};
			if (transformation) options.transformation = transformation;

			return new L.Proj.CRS(code, def, options);
		};
	}());
}

return L.Proj;

}));
