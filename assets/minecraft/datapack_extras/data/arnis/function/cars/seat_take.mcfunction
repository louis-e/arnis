# One rider to a car.
execute if entity @s[tag=arnis.car.ridden] run return 0
data modify entity @s data.f set from storage arnis:cars seat.f
data modify entity @s data.u set from storage arnis:cars seat.u
function arnis:cars/seat_new with storage arnis:cars seat
