# Once a second, a few tries at bringing back a car that left.
scoreboard players set #car_r arnis 0
execute if score #segs arnis.car matches 1.. if score #pending arnis.car matches 1.. run function arnis:cars/respawn_try
execute if score #segs arnis.car matches 1.. if score #pending arnis.car matches 1.. run function arnis:cars/respawn_try
execute if score #segs arnis.car matches 1.. if score #pending arnis.car matches 1.. run function arnis:cars/respawn_try
