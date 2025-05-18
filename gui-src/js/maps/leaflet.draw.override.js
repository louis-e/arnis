// Custom override for Leaflet.Draw to fix the rectangle drawing behavior
// This file needs to be loaded after leaflet.draw.js

// Store original methods to call them later when needed
L.Draw.SimpleShape.prototype._originalMouseDown = L.Draw.SimpleShape.prototype._onMouseDown;
L.Draw.SimpleShape.prototype._originalMouseMove = L.Draw.SimpleShape.prototype._onMouseMove;
L.Draw.SimpleShape.prototype._originalMouseUp = L.Draw.SimpleShape.prototype._onMouseUp;

// Fix for Rectangle drawing behavior
if (L.Draw.Rectangle) {
    // Override the mouse down handler - keep original functionality but we'll modify behavior in move and up
    L.Draw.SimpleShape.prototype._onMouseDown = function(e) {
        if (this.type === "rectangle") {
            // For rectangle, use original behavior
            this._isDrawing = true;
            this._startLatLng = e.latlng;
            
            L.DomEvent
                .on(document, 'mouseup', this._onMouseUp, this)
                .preventDefault(e.originalEvent);
        } else {
            // For other shapes, use original behavior
            this._originalMouseDown.call(this, e);
        }
    };

    // Override mouse move to maintain proper tooltip
    L.Draw.SimpleShape.prototype._onMouseMove = function(e) {
        var latlng = e.latlng;
        
        this._tooltip.updatePosition(latlng);
        
        if (this._isDrawing) {
            if (this.type === "rectangle") {
                // Keep showing the initial tooltip while drawing
                this._tooltip.updateContent({
                    text: L.drawLocal.draw.handlers.rectangle.tooltip.start
                });
            } else {
                // For other shapes, use original behavior
                this._tooltip.updateContent({
                    text: this._endLabelText
                });
            }
            
            this._drawShape(latlng);
        }
    };    // Override mouse up to ensure drawing stops properly
    L.Draw.SimpleShape.prototype._onMouseUp = function() {
        if (this._shape && this.type === "rectangle") {
            // For rectangle, finish drawing on mouse up
            this._fireCreatedEvent();
            this.disable();
        } else if (this._originalMouseUp) {
            // For other shapes, use original behavior
            this._originalMouseUp.call(this);
        }
    };
}
