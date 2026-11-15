# A level crossing's stop too close past the junction ahead to wait at clear of it: while
# the crossing is closed the car waits before the junction instead.
$execute if score #x$(gx) arnis.xing matches 1.. run function arnis:cars/gated
