wp.getMapFormat()
    .withId('org.pepsoft.anvil.1.19')
    .go();

var project = arguments[0];
var exportDirectory = arguments[1];
var world = wp.getWorld().fromFile(project).go();
wp.exportWorld(world).toDirectory(exportDirectory).go();
