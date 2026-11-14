# The next lane, joined at the first of its points ahead of the car, and whether getting
# there crosses a junction.
data remove entity @s data.gr
data remove entity @s data.gc
data remove entity @s data.gl
data remove entity @s data.gy
data remove entity @s data.gb
$data modify entity @s data merge from storage arnis:cars seg[$(s)].n[$(k)]
# Across a junction: before setting off, room past it (gate). Others tell it is crossing.
execute if data entity @s data.j run tag @s add arnis.car.gate
execute if data entity @s data.j run tag @s add arnis.car.cross
function arnis:cars/enter with entity @s data
